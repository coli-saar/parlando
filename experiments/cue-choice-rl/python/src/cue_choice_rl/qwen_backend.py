"""LoRA policy/value model and clipped PPO updates for Apple Silicon or CPU."""

from __future__ import annotations

from pathlib import Path
from typing import Any

from .runtime import canonical_prompt, resolve_checkpoint_directory


class QwenPpoBackend:
    """Checkpoint-aware Qwen encoder with a four-way policy and scalar value head."""

    CHOICES = ("oak", "pine", "birch", "elm")

    def __init__(self, model_name: str, requested_device: str) -> None:
        """Loads all heavyweight dependencies and the base model before serving."""
        try:
            import torch
            from peft import LoraConfig, TaskType, get_peft_model
            from transformers import AutoModel, AutoTokenizer
        except ImportError as exc:
            raise RuntimeError(
                "Qwen mode requires `pip install -e '.[qwen]'`"
            ) from exc
        self.torch = torch
        self.model_name = model_name
        self.device = self._select_device(requested_device)
        self.tokenizer = AutoTokenizer.from_pretrained(model_name)
        base = AutoModel.from_pretrained(
            model_name, dtype=torch.float32, low_cpu_mem_usage=True
        )
        config = LoraConfig(
            task_type=TaskType.FEATURE_EXTRACTION,
            r=8,
            lora_alpha=16,
            lora_dropout=0.0,
            target_modules=("q_proj", "v_proj"),
        )
        self.encoder = get_peft_model(base, config).to(self.device)
        hidden = int(base.config.hidden_size)
        self.policy_head = torch.nn.Linear(hidden, len(self.CHOICES)).to(self.device)
        self.value_head = torch.nn.Linear(hidden, 1).to(self.device)
        torch.nn.init.zeros_(self.policy_head.weight)
        torch.nn.init.zeros_(self.policy_head.bias)
        torch.nn.init.zeros_(self.value_head.weight)
        torch.nn.init.zeros_(self.value_head.bias)
        self._checkpoints = {"initial": self._snapshot()}
        self._updates: dict[str, str] = {}

    def _select_device(self, requested: str) -> Any:
        """Selects MPS on a capable Mac, with an explicit and predictable CPU fallback."""
        torch = self.torch
        mps = bool(torch.backends.mps.is_available())
        if requested == "mps" and not mps:
            raise RuntimeError("MPS is unavailable; start the learner with --device cpu")
        return torch.device("mps" if requested == "mps" or requested == "auto" and mps else "cpu")

    def _trainable_parameters(self) -> list[Any]:
        """Returns LoRA and task-head parameters while excluding frozen base weights."""
        return [
            *[parameter for parameter in self.encoder.parameters() if parameter.requires_grad],
            *self.policy_head.parameters(),
            *self.value_head.parameters(),
        ]

    def _snapshot(self) -> dict[str, Any]:
        """Copies only trainable tensors to CPU as one semantically immutable checkpoint."""
        return {
            name: tensor.detach().cpu().clone()
            for name, tensor in self._trainable_state().items()
        }

    def _trainable_state(self) -> dict[str, Any]:
        """Names trainable encoder and head tensors in one flat checkpoint namespace."""
        state = {
            f"encoder.{name}": parameter
            for name, parameter in self.encoder.named_parameters()
            if parameter.requires_grad
        }
        state.update(
            {f"policy.{name}": value for name, value in self.policy_head.state_dict().items()}
        )
        state.update(
            {f"value.{name}": value for name, value in self.value_head.state_dict().items()}
        )
        return state

    def _restore(self, checkpoint_id: str) -> None:
        """Restores one in-memory checkpoint into the shared inference model."""
        source = self._checkpoints.get(checkpoint_id)
        if source is None:
            raise ValueError(f"unknown checkpoint: {checkpoint_id}")
        for name, target in self._trainable_state().items():
            target.data.copy_(source[name].to(self.device))

    def _forward(self, observations: list[dict[str, Any]]) -> tuple[Any, Any]:
        """Encodes canonical prompts and returns policy logits and values."""
        encoded = self.tokenizer(
            [canonical_prompt(item) for item in observations],
            padding=True,
            truncation=True,
            max_length=128,
            return_tensors="pt",
        ).to(self.device)
        output = self.encoder(**encoded)
        lengths = encoded["attention_mask"].sum(dim=1) - 1
        rows = self.torch.arange(len(observations), device=self.device)
        pooled = output.last_hidden_state[rows, lengths]
        return self.policy_head(pooled), self.value_head(pooled).squeeze(-1)

    def choose(
        self,
        checkpoint_id: str,
        observation: dict[str, Any],
        actions: list[dict[str, Any]],
        seed: int,
        temperature: float,
    ) -> dict[str, Any]:
        """Samples among runner-authorized choices using the pinned checkpoint."""
        self._restore(checkpoint_id)
        self.encoder.eval()
        with self.torch.no_grad():
            logits, _value = self._forward([observation])
        allowed = {action["choice"]: action for action in actions}
        indices = [self.CHOICES.index(choice) for choice in allowed]
        probabilities = self.torch.softmax(logits[0, indices] / temperature, dim=-1).cpu()
        generator = self.torch.Generator(device="cpu").manual_seed(seed)
        selected = int(self.torch.multinomial(probabilities, 1, generator=generator).item())
        return allowed[self.CHOICES[indices[selected]]]

    def train(
        self, update_id: str, base_checkpoint_id: str, steps: list[dict[str, Any]], settings: dict[str, Any]
    ) -> str:
        """Runs clipped PPO over the runner's accepted learner transitions."""
        prior = self._updates.get(update_id)
        if prior is not None:
            return prior
        self._validate_settings(settings)
        self._restore(base_checkpoint_id)
        observations, actions, rewards = self._training_tensors(steps)
        torch = self.torch
        self.encoder.eval()
        with torch.no_grad():
            old_logits, old_values = self._forward(observations)
            old_log_probs = torch.log_softmax(old_logits, dim=-1)[
                torch.arange(len(actions), device=self.device), actions
            ]
        advantages = self._advantages(old_values, rewards)
        optimizer = torch.optim.AdamW(
            self._trainable_parameters(), lr=float(settings["learning_rate"])
        )
        indices = torch.arange(len(actions), device=self.device)
        batch_size = int(settings["minibatch_size"])
        for _epoch in range(int(settings["ppo_epochs"])):
            for batch in indices.split(batch_size):
                logits, values = self._forward([observations[int(index)] for index in batch])
                selected = actions[batch]
                log_probs = torch.log_softmax(logits, dim=-1)[
                    torch.arange(len(batch), device=self.device), selected
                ]
                ratio = (log_probs - old_log_probs[batch]).exp()
                unclipped = ratio * advantages[batch]
                clipped = ratio.clamp(0.8, 1.2) * advantages[batch]
                policy_loss = -torch.minimum(unclipped, clipped).mean()
                value_loss = 0.5 * (values - rewards[batch]).square().mean()
                entropy = -(torch.softmax(logits, -1) * torch.log_softmax(logits, -1)).sum(-1).mean()
                loss = policy_loss + value_loss - 0.01 * entropy
                optimizer.zero_grad(set_to_none=True)
                loss.backward()
                torch.nn.utils.clip_grad_norm_(self._trainable_parameters(), 1.0)
                optimizer.step()
        checkpoint = f"cue-qwen25/update-{len(self._updates) + 1:06d}"
        self._checkpoints[checkpoint] = self._snapshot()
        self._updates[update_id] = checkpoint
        cadence = int(settings["save_every_checkpoints"])
        if len(self._updates) % cadence == 0:
            self._save(checkpoint, settings["checkpoint_directory"])
        return checkpoint

    def _advantages(self, values: Any, rewards: Any) -> Any:
        """Uses TorchRL's GAE utility for this one-step terminal environment."""
        from torchrl.objectives.value.functional import generalized_advantage_estimate

        shape = (-1, 1, 1)
        advantage, _target = generalized_advantage_estimate(
            gamma=1.0,
            lmbda=1.0,
            state_value=values.reshape(shape),
            next_state_value=self.torch.zeros_like(values).reshape(shape),
            reward=rewards.reshape(shape),
            done=self.torch.ones_like(rewards, dtype=self.torch.bool).reshape(shape),
            terminated=self.torch.ones_like(rewards, dtype=self.torch.bool).reshape(shape),
            time_dim=1,
        )
        advantage = advantage.reshape(-1)
        return (advantage - advantage.mean()) / advantage.std(unbiased=False).clamp_min(1e-6)

    def _training_tensors(self, steps: list[dict[str, Any]]) -> tuple[list[dict[str, Any]], Any, Any]:
        """Decodes the language-neutral trajectory envelope into policy tensors."""
        observations: list[dict[str, Any]] = []
        choices: list[int] = []
        rewards: list[float] = []
        for step in steps:
            if step["role"] != "A" or not step["accepted"]:
                continue
            observation = step["observation"]
            action = step["action"]
            observations.append(observation)
            choices.append(self.CHOICES.index(action["choice"]))
            rewards.append(float(step["rewards"]["player_a"]))
        if not observations:
            raise ValueError("training update contains no accepted player-A transitions")
        return (
            observations,
            self.torch.tensor(choices, dtype=self.torch.long, device=self.device),
            self.torch.tensor(rewards, dtype=self.torch.float32, device=self.device),
        )

    def _validate_settings(self, settings: dict[str, Any]) -> None:
        """Ensures process-level model choices agree with the experiment YAML."""
        if settings.get("model") != self.model_name:
            raise ValueError("YAML model does not match the preloaded service model")
        configured = settings.get("device", "mps")
        if configured != "auto" and configured != self.device.type:
            raise ValueError("YAML device does not match the service device")
        if int(settings.get("lora_rank", 8)) != 8:
            raise ValueError("this preloaded v1 backend requires lora_rank: 8")

    def _save(self, checkpoint_id: str, directory: str) -> None:
        """Persists learner-owned checkpoint tensors at the configured cadence."""
        target: Path = resolve_checkpoint_directory(directory) / f"{checkpoint_id.replace('/', '-')}.pt"
        temporary = target.with_suffix(".tmp")
        self.torch.save(self._checkpoints[checkpoint_id], temporary)
        temporary.replace(target)
