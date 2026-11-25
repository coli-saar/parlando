# Train a Parlando agent with OpenEnv

An OpenEnv trainer controls one seat in a Parlando game. Each call to `step_async` submits that seat's decision and runs the game and opponent until the learner can decide again or the episode ends. The game package determines which actions are legal and which view each seat receives. The opponent can run in the same process as a WASM component or behind a Parlando gRPC endpoint.

## Run a first episode

From the repository root, use a Python virtual environment with Rust and Cargo available to build the extension. Install the two local Python packages together.

```sh
python -m pip install ./parlando-agent-sdk './parlando-rl[openenv]'
./experiments/cue-choice-rl/build.sh
```

In cue-choice, seat B deals and seat A chooses. The example below plays the first available action and prints the game's terminal result. Changing the choice can change the reward.

```python
import asyncio
from pathlib import Path

from parlando_rl.openenv import ParlandoAction, ParlandoEnvironment


async def play():
    packages = Path("experiments/cue-choice-rl/dist")
    env = ParlandoEnvironment(
        game=packages / "cue-choice-0.1.0.zip",
        opponent={
            "factory": "wasm",
            "path": packages / "agent-cue_choice.dealer-1.0.0.zip",
        },
        learner_role="A",
        schedule={"kind": "alternate_after_action", "first": "player_b"},
    )
    try:
        obs = await env.reset_async(seed=17, config={"level": "dax"})
        while not obs.done:
            choice = obs.view["available_actions"][0]
            obs = await env.step_async(ParlandoAction(action=choice))
        print(obs.reward, obs.metadata.get("completion"))
    finally:
        await env.close_async()


asyncio.run(play())
```

A rollout owns one environment instance. Create one per concurrent rollout and await their steps independently. Pass the same seed, scenario, and deterministic opponent to reproduce an opening. A remote model that samples stochastically also needs to honor its session seed for this to work.

## Read deliveries and choose actions

At reset, `initial_view` is what the learner's agent receives in its `start` callback. The opponent may act before the first learner decision. In that case `events` already contains the resulting deliveries, and `view` reflects them. Subsequent observations contain just the events since the preceding decision. A transition carries the actor, the action and the learner's resulting view. An attached message follows the transition and reaches its recipient. Values use the remote agent SDK's protobuf representation, including double-precision numbers.

Submit a game action, a chat message, both, or an empty `ParlandoAction()` to yield. Inspect `obs.reason.rejection_code` when the reason is `action_rejected`. The game supplies the code so the learner can revise its next decision. End the rollout when `done` is true, including when a session limit ended play before the game produced a completion.

Use `alternate_after_action` to reproduce the headless turn schedule. Use `live` to train an agent that must respond to deliveries or yield while waiting for a human partner. A headless agent requesting a timed wakeup fails the episode.

## Configure scenarios, opponents and rewards

Supply a game's configuration dictionary directly to reset, or declare a mapping such as `scenario_configs={"intro": {"level": "dax"}}` and call `reset_async(seed=17, config="intro")`. The opponent receives the reset seed in its SDK context. When evaluating with `parlando run`, the runner derives separate game and agent seeds for each session plan. For policies that use randomness in their own decisions, record the agent context seed as well as the game seed.

Set `opponent` to one of the following configurations.

| Opponent | Configuration |
| --- | --- |
| Packaged WASM agent | `{"factory": "wasm", "path": dealer_zip}` |
| Existing gRPC endpoint | `{"factory": "remote_grpc", "endpoint": endpoint_url, "settings": agent_settings}` |
| Python SDK agent in the training process | `{"factory": "python", "source": agent_factory, "settings": agent_settings}` |

The Python factory runs on a temporary loopback gRPC listener. Use this form on a trusted host. To share an opponent across processes, run it as a regular gRPC service. Use the [remote agent transport settings](agents.md) for authenticated remote endpoints.

The default reward uses the game's numeric `completion.reward` or a Boolean `success` or `correct` field. Games with other completion formats can supply a Python function through the environment's `reward` argument. Each accepted action supplies a `RewardContext` with the actor, action, completion, and both seats' views before and after the move. Its return value contributes to the step's total reward. The learner receives that scalar and its own deliveries. The environment owner can inspect opponent logs through `env.opponent_logs`. Only the learner's own `AgentPolicy` log entries reach its observation metadata.

Use `limits` to configure `decisions`, `accepted_actions`, `messages`, `rejected_actions`, `callback_timeout_seconds`, `shutdown_timeout_seconds` and `session_timeout_seconds`. The counters cover both seats. `wasm_metering` defaults to `fuel`; select `none` for trusted components when you want to disable fuel accounting.

## Serve an environment over WebSocket

Save the following as `environment.yaml` in the repository root. The server resolves package paths relative to this file.

```yaml
game: experiments/cue-choice-rl/dist/cue-choice-0.1.0.zip
opponent:
  factory: wasm
  path: experiments/cue-choice-rl/dist/agent-cue_choice.dealer-1.0.0.zip
learner_role: A
schedule:
  kind: alternate_after_action
  first: player_b
scenario_configs:
  intro: {level: dax}
```

Run `PARLANDO_OPENENV_CONFIG=environment.yaml server` from the Python environment where you installed the SDK. The default port is 8000. Each WebSocket connection owns a separate episode. Set `PARLANDO_OPENENV_MAX_SESSIONS` to configure simultaneous connections. HTTP reset and step requests each create a fresh environment, so use one persistent client for a multi-step episode.

```python
import asyncio
from parlando_rl.openenv import ParlandoAction, ParlandoClient


async def play_remote():
    async with ParlandoClient(base_url="http://127.0.0.1:8000") as client:
        result = await client.reset(seed=17, config="intro")
        while not result.done:
            choice = result.observation.view["available_actions"][0]
            result = await client.step(ParlandoAction(action=choice))
        print(result.observation.metadata["completion"])


asyncio.run(play_remote())
```

To deploy from a checkout, install `uv` and stage the source dependencies and the selected packages in a new directory. Supply a server configuration suitable for publishing. OpenEnv uploads the staged configuration and game archives along with the code.

```sh
python parlando-rl/scripts/stage_server.py environment.yaml /tmp/cue-choice-openenv
openenv validate /tmp/cue-choice-openenv
```

The staged directory is the input to `openenv push`. To build the same Docker image locally, use `docker build -f /tmp/cue-choice-openenv/server/Dockerfile /tmp/cue-choice-openenv`.

## Train a checkpoint and evaluate it with people

The cue-choice walkthrough trains a small language model on CPU with Hugging Face TRL's `GRPOTrainer`. TRL samples game actions through persistent OpenEnv WebSocket sessions, and the Parlando runner evaluates the saved checkpoint through gRPC. The walkthrough also includes an in-process tabular baseline. [Train and evaluate a cue-choice agent](../parlando-rl/examples/cue_choice/README.md)

A model-based agent follows the same deployment path. Keep the observation rendering and decision code in a module that imports `parlando_agent_sdk`. During training, either call it directly from the trainer or use `AgentPolicy` to drive its normal callbacks. For a policy in the loop, construct a fresh `SessionInfo` with the learner's role, seed and settings, then call `reset`, `act` for each pending decision, and `finish` on the terminal observation. The policy delivers opening events during reset and terminal events before shutdown.

For evaluation, install the agent SDK and the model's own dependencies in the serving process. Supply the checkpoint through the agent settings and register its endpoint as a `remote_grpc` seat. The same endpoint can then participate in a human–agent experiment. See the [agent guide](agents.md) and [headless experiment guide](agent-agent-and-rl-experiments.md).
