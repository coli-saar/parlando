# Train and deploy policies with Parlando RL

A Parlando game determines what each player can see, which decisions it accepts, and when play ends. A `ParlandoEpisode` runs one seat against a configured opponent using the native library in the `parlando-rl` wheel. Choose an adapter for the learner. Each adapter can run in a separate Python process with its own game, opponent, and dependencies.

## Train a Gymnasium policy and play it in Parlando

From the repository root, build the cue-choice packages, install the SDK and Stable-Baselines3, and train a PPO policy on CPU. Building the Python wheel from this checkout requires the Parlando Rust build toolchain.

```sh
./experiments/cue-choice-rl/build.sh
python -m pip install ./parlando-agent-sdk './parlando-rl[sb3]'
python parlando-rl/examples/cue_choice/train_sb3.py train --checkpoint cue-choice-ppo.zip
python parlando-rl/examples/cue_choice/train_sb3.py serve cue-choice-ppo.zip
```

The learner sees a four-element vector identifying its cue. Each of its four actions names one possible choice. The example reports accuracy for a deterministic choice on each cue before and after training. The final command loads the saved weights into an ordinary SDK agent and listens on port 50061. In a second terminal, run the same game with the Parlando experiment runner. Use a fresh directory for the results.

```sh
repo="$PWD"
run_dir=$(mktemp -d)
cp parlando-rl/examples/cue_choice/eval.yaml "$run_dir/"
(cd "$run_dir" && parlando run eval.yaml \
  --package "$repo/experiments/cue-choice-rl/dist/cue-choice-0.1.0.zip" \
  --agent "$repo/experiments/cue-choice-rl/dist/agent-cue_choice.dealer-1.0.0.zip")
```

To expose another game, give `ParlandoGymEnv` its observation and action spaces, a function that encodes the learner's role-private frame (view, new events, and decision reason), and a function that converts a model action into a Parlando decision. Use the same functions with `GymPolicyAgent` when loading the checkpoint. A game completion sets Gymnasium's `terminated` flag. A session limit sets `truncated`. An episode that ends during reset has no learner decision and raises `ResetNeeded`. [Cue-choice PPO example](../parlando-rl/examples/cue_choice/train_sb3.py)

## Choose the policy interface

| Framework | Use a Parlando SDK agent as a policy | Play the learned policy in Parlando |
| --- | --- | --- |
| OpenEnv | Give `parlando_rl.openenv.AgentPolicy` a factory and `SessionInfo`. Pass the opening observation to `reset`, each decision observation to `act`, and the terminal observation to `finish`. | Construct a `PolicyAgent` with a callable that takes the current private view, decision reason, and new events and returns a `ParlandoAction`. |
| Gymnasium | `GymAgentPolicy` drives an SDK agent from the environment's full `parlando_observation` and encodes its choices into the declared action space. This preserves messages and callbacks that a numeric observation may omit. | `GymPolicyAgent` wraps a model with a Stable-Baselines3-style `predict` method. Use a fresh agent instance for each seat and session. |
| Verifiers | Use a Parlando agent as an evaluation policy on the same tasks scored for a chat model. | `ParlandoModelAgent` accepts an asynchronous chat-completion function for a checkpoint. Use the same instructions and decision format during training and deployment. |

A checkpoint belongs to its trainer. Load the weights with that trainer or inference backend and supply the resulting policy in a normal SDK agent factory. Live agents receive only the information delivered to their seat. Training rewards and private opponent state stay with the episode. See the [OpenEnv guide](training-with-openenv.md) for serving an environment or training through a WebSocket.

## Evaluate a dialogue policy with Verifiers

The Last Screening gives the stagehand and the projectionist different cues. A stagehand policy can send a message and make a move in one decision. The example presents each decision as a JSON chat message containing the stagehand's own view and new events. Both players escaping earns one point.

Use Python 3.11 or newer with `uv` available to the Verifiers worker, and run the following from the repository root. The scripted SDK stagehand exercises the standard Verifiers evaluation path locally.

```sh
./games/last-screening/build.sh
python -m pip install ./parlando-agent-sdk './parlando-rl[verifiers]'
example=parlando-rl/examples/last_screening
PYTHONPATH="$example" vf-run @ "$example/eval.toml" \
  --env.sdk_agent last_screening_verifiers:Stagehand
```

For a game whose completion already supplies a numeric reward or a Boolean success flag, use the installed taskset ID `parlando_verifiers` with game and opponent paths in the Verifiers configuration. The Last Screening example supplies a custom reward for its `escaped` result.

For a model policy, omit the SDK factory and select the model and endpoint through Verifiers. Model replies use optional `action` and `message` fields. The environment records the game's reward on the resulting Verifiers trace. An SDK evaluation records the decisions and reward without token probabilities. To play a trained chat checkpoint in Parlando, point `OPENAI_BASE_URL`, `OPENAI_API_KEY`, and `PARLANDO_MODEL` at its inference service and run `PYTHONPATH="$example" python -m last_screening_verifiers`. The SDK factory uses the same instructions and formats the same role-private messages. [Dialogue example](../parlando-rl/examples/last_screening/last_screening_verifiers.py)
