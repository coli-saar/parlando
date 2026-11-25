"""Train a Parlando agent with GRPO against a served Cue Choice game."""

import argparse
import asyncio
import json
from contextlib import ExitStack
from functools import partial
from pathlib import Path

import torch
from parlando_agent_sdk import Agent, Decision
from tokenizers import Tokenizer
from tokenizers.models import WordLevel
from tokenizers.pre_tokenizers import WhitespaceSplit
from transformers import GPT2Config, GPT2LMHeadModel, PreTrainedTokenizerFast

CUES = ("dax", "wug", "kiki", "zorp")
CHOICES = ("oak", "pine", "birch", "elm")


class CueChoiceAgent(Agent):
    """Play a cue-conditioned language-model policy as a Parlando agent.

    For a prompt such as "cue dax choice", the model scores four possible next
    tokens: oak, pine, birch, and elm. During training, TRL samples a token
    and passes it in through select(view). GRPO updates the model weights to
    favor choices earning higher rewards among samples for that same cue.
    At evaluation, select(view) runs the saved model and takes the legal token
    with the highest score. The agent maps that token to the offered action.
    """

    def __init__(self, select):
        """Use TRL's sampled choice during training or the checkpoint's greedy choice."""
        self.select = select

    async def start(self, view):
        """Remember the initial observation."""
        self.view = view

    async def observe_transition(self, actor, action, observation):
        """Use the updated view after the dealer acts."""
        self.view = observation

    async def decide(self, reason):
        """Yield while waiting, otherwise submit the selected legal action."""
        actions = {action["choice"]: action for action in self.view.get("available_actions", [])}
        if not actions:
            return Decision()
        chosen_token = self.select(self.view)
        return Decision(action=actions[chosen_token])


def prompt(view):
    """Render the same cue for training and checkpoint inference."""
    return f"cue {view['cue']} choice"


@torch.inference_mode()
def greedy_choice(model, tokenizer, view):
    """Select the legal choice with the highest next-token score."""
    inputs = tokenizer.apply_chat_template([{"role": "user", "content": prompt(view)}],
                                           add_generation_prompt=True, return_dict=True, return_tensors="pt")
    choices = [action["choice"] for action in view["available_actions"]]
    logits = model(**inputs).logits[0, -1, tokenizer.convert_tokens_to_ids(choices)]
    return choices[int(logits.argmax())]


def new_model():
    """Build a one-layer CPU model with one token per cue and choice."""
    words = ("<pad>", "<eos>", "<unk>", "cue", "choice", *CUES, *CHOICES)
    backend = Tokenizer(WordLevel({word: i for i, word in enumerate(words)}, unk_token="<unk>"))
    backend.pre_tokenizer = WhitespaceSplit()
    # TRL requires a chat template that also renders tool calls.
    tokenizer = PreTrainedTokenizerFast(
        tokenizer_object=backend, pad_token="<pad>", eos_token="<eos>", unk_token="<unk>",
        chat_template="{% for m in messages %}{{ m.content | default('') }} "
                      "{% for c in m.tool_calls | default([]) %}{{ c.function.name }} "
                      "{{ c.function.arguments | tojson }} {% endfor %}{% endfor %}",
    )
    config = GPT2Config(vocab_size=len(words), n_positions=8, n_embd=32, n_layer=1, n_head=2,
                        resid_pdrop=0., embd_pdrop=0., attn_pdrop=0., bos_token_id=None,
                        eos_token_id=tokenizer.eos_token_id, pad_token_id=tokenizer.pad_token_id)
    return GPT2LMHeadModel(config), tokenizer


class CueChoiceEnvironment:
    """One Parlando game session. TRL resets it, then scores an agent decision in it."""

    def __init__(self, client, runner):
        """Keep this environment's WebSocket on the trainer's event loop."""
        self.client, self.runner = client, runner

    def reset(self, episode_seed, level):
        """Start the game and give TRL the cue revealed by the WASM dealer."""
        self.seed = episode_seed
        result = self.runner.run(self.client.reset(seed=episode_seed, config={"level": level}))
        self.obs = result.observation
        if result.done or {a["choice"] for a in self.obs.view.get("available_actions", [])} != set(CHOICES):
            raise ValueError("Expected the dealer to offer all four choices")
        return prompt(self.obs.view)

    async def _play(self, select):
        """Drive an ordinary SDK agent through its opening, decision and terminal callbacks."""
        from parlando_rl.openenv import AgentPolicy, SessionInfo

        policy = AgentPolicy(lambda ctx: CueChoiceAgent(select), SessionInfo("A", self.seed))
        await policy.reset(self.obs)
        try:
            decision = await policy.act(self.obs)
            result = await self.client.step(decision)
            if not result.done or "completion" not in result.observation.metadata:
                raise RuntimeError("Cue Choice must complete after one legal choice")
            await policy.finish(result.observation)
            return decision.action["choice"], result.reward
        finally:
            await policy.close()


def evaluate(env, select):
    """Record greedy decisions on the same sixteen games as the gRPC evaluation."""
    results = []
    for cue in CUES:
        for seed in range(10_001, 10_005):
            env.reset(seed, cue)
            choice, reward = env.runner.run(env._play(select))
            results.append(dict(cue=cue, seed=seed, choice=choice, reward=reward))
    return results


def train(url, output, steps, seed):
    """Fit the agent on eight choices per cue and compare the two checkpoints."""
    from datasets import Dataset
    from parlando_rl.openenv import ParlandoClient
    from transformers import set_seed
    from trl import GRPOConfig, GRPOTrainer

    torch.set_num_threads(1)
    set_seed(seed)
    model, tokenizer = new_model()
    select = partial(greedy_choice, model, tokenizer)
    rows = [{"episode_seed": 20_000 + i, "level": CUES[i % len(CUES)]} for i in range(max(steps, len(CUES)))]
    config = GRPOConfig(
        output_dir=str(output), max_steps=steps, seed=seed, use_cpu=True, bf16=False, fp16=False,
        num_generations=8, per_device_train_batch_size=8, max_completion_length=1,
        generation_kwargs={"suppress_tokens": [i for word, i in tokenizer.get_vocab().items() if word not in CHOICES]},
        learning_rate=0.003, lr_scheduler_type="constant", beta=0., report_to="none",
        logging_steps=4, logging_first_step=True, save_steps=max(1, steps // 2),
    )
    with asyncio.Runner() as runner, ExitStack() as sessions:
        def environment():
            """Give each sample its own connection to the Parlando game."""
            client = ParlandoClient(base_url=url)
            runner.run(client.connect())
            sessions.callback(lambda: runner.run(client.disconnect()))
            return CueChoiceEnvironment(client, runner)

        async def score_sample(env, reply):
            """Let the agent play TRL's sampled token in the game that supplied its cue."""
            choice = reply[0]["content"].strip()
            _, reward = await env._play(lambda view: choice)
            return reward

        async def scores(samples):
            """Step independent games concurrently."""
            return await asyncio.gather(*(score_sample(env, reply) for env, reply in samples))

        def game_reward(environments, completions, **kwargs):
            """Return one game reward for each GRPO completion."""
            return runner.run(scores(zip(environments, completions, strict=True)))

        test_env = environment()
        model.eval().save_pretrained(output / "initial")
        tokenizer.save_pretrained(output / "initial")
        initial = evaluate(test_env, select)
        trainer = GRPOTrainer(model=model, processing_class=tokenizer, train_dataset=Dataset.from_list(rows),
                              environment_factory=environment, reward_funcs=game_reward, args=config)
        trainer.train()
        trainer.save_model(str(output / "final"))
        model.eval()
        final = evaluate(test_env, select)

    history = [{"step": row["step"], "mean_reward": row["reward"]}
               for row in trainer.state.log_history if "reward" in row]
    report = dict(seed=seed, num_generations=config.num_generations,
                  training=history, held_out={"initial": initial, "final": final})
    (output / "learning-curve.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    for label, games in report["held_out"].items():
        print(f"Held-out {label} greedy reward: {sum(game['reward'] for game in games) / len(games):.3f}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", default="http://127.0.0.1:8000")
    parser.add_argument("--output-dir", type=Path, default=Path("outputs/cue-choice-grpo"))
    parser.add_argument("--steps", type=int, default=128)
    parser.add_argument("--seed", type=int, default=0)
    args = parser.parse_args()
    if args.steps < 1:
        parser.error("--steps must be positive")
    train(args.url, args.output_dir, args.steps, args.seed)
