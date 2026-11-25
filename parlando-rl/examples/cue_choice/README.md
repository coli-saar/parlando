# Train and evaluate a cue-choice agent

In Cue Choice, a WASM dealer reveals a cue to the learner. The learner chooses
one of four options and receives a reward of one for a correct choice or zero
otherwise. The example includes a CPU language-model trainer that uses
OpenEnv over WebSocket and a tabular bandit that runs the game in-process.
Both checkpoints can be evaluated by the same Parlando experiment.

## Train a language model with TRL and OpenEnv

From the repository root, install the two local SDKs and the example's trainer
dependencies in a Python 3.12 environment. Build the cue-choice packages with
the Parlando CLI, the Rust `wasm32-unknown-unknown` target and `wasm-tools`
available. Start the environment server and leave it running.

```sh
python -m pip install ./parlando-agent-sdk './parlando-rl[openenv]' -r parlando-rl/examples/cue_choice/requirements-grpo.txt
experiments/cue-choice-rl/build.sh
PARLANDO_OPENENV_CONFIG=parlando-rl/examples/cue_choice/environment.yaml server
```

Run the trainer in a second terminal using the same Python environment.

```sh
python parlando-rl/examples/cue_choice/train_grpo.py
```

For a cue such as `dax`, the model reads `cue dax choice` and scores four
possible next tokens: `oak`, `pine`, `birch`, and `elm`. The four scores define
the policy's distribution over actions for that cue. TRL samples eight times
from this distribution, using the same cue and seed in eight OpenEnv sessions.
In each session, `AgentPolicy` drives a `CueChoiceAgent`, which looks up the
sampled token in the actions the game offered and submits the matching action.

Parlando returns 1 for a correct choice and 0 otherwise. When samples for the
same cue earn different rewards, GRPO updates the initially random model
weights to make the higher-reward choices more likely. The saved weights carry
the learned cue-to-choice mapping. At evaluation, the agent plays the legal
token with the highest score from that checkpoint.

The upstream example uses the same trainer hook for Echo. [TRL OpenEnv guide](https://huggingface.co/docs/trl/openenv)

By default the trainer runs 128 updates and saves `initial/`, `final/` and
`learning-curve.json` under `outputs/cue-choice-grpo/`. The JSON records
sampled training rewards and each checkpoint's greedy choice on four held-out
seeds per cue. These seeds test repeatability and transfer through the serving
path. The four cue meanings also appear in training. With the pinned
dependencies and default model seed, greedy play scored 4/16 before training
and 16/16 after training. For a one-update check with both successful and
unsuccessful choices in the sampled group, use the following command.

```sh
python parlando-rl/examples/cue_choice/train_grpo.py --steps 1 --seed 1 --output-dir outputs/cue-choice-smoke
```

To evaluate the learned checkpoint with Parlando, start its SDK agent.

```sh
python parlando-rl/examples/cue_choice/checkpoint_agent.py outputs/cue-choice-grpo/final
```

The serving process needs the agent SDK, PyTorch, and Transformers. Run the
following from the repository root in another terminal. A fresh working
directory keeps this evaluation separate from previous runner results.

```sh
repo="$PWD"
run_dir=$(mktemp -d)
cp parlando-rl/examples/cue_choice/eval.yaml "$run_dir/"
(cd "$run_dir" && parlando run eval.yaml \
  --package "$repo/experiments/cue-choice-rl/dist/cue-choice-0.1.0.zip" \
  --agent "$repo/experiments/cue-choice-rl/dist/agent-cue_choice.dealer-1.0.0.zip")
```

Inspect `completion.correct` in `$run_dir/cue-choice-results/results/*.json`.
To evaluate the initial checkpoint, restart the SDK agent with the `initial`
directory and repeat in another fresh working directory.

## Train the table

Install [Parlando OpenEnv](../../../docs/training-with-openenv.md) and make the
`parlando` CLI available on your PATH. Building the game requires the Rust
`wasm32-unknown-unknown` target and `wasm-tools`. From the repository root,
build the game and dealer packages and train the policy.

```bash
experiments/cue-choice-rl/build.sh
python parlando-rl/examples/cue_choice/train.py \
  --game experiments/cue-choice-rl/dist/cue-choice-0.1.0.zip \
  --dealer experiments/cue-choice-rl/dist/agent-cue_choice.dealer-1.0.0.zip
```

The bandit alternates among the four cue scenarios. It explores a random legal
action on 10% of episodes and otherwise uses the action with the highest
estimated reward for the cue. The default 5,000 episodes produce a table with
an estimated mean reward of 1.000.

## Evaluate the table with Parlando

From the directory containing `table.json`, serve the agent. The serving Python
environment only needs the Parlando agent SDK (`parlando-remote-agent`).

```bash
python parlando-rl/examples/cue_choice/agent.py
```

Keep the server running and start the evaluation from the repository root in
another terminal.

```bash
parlando run parlando-rl/examples/cue_choice/eval.yaml \
  --package experiments/cue-choice-rl/dist/cue-choice-0.1.0.zip \
  --agent experiments/cue-choice-rl/dist/agent-cue_choice.dealer-1.0.0.zip
```

The specification puts the remote agent in seat A and the same WASM dealer in
seat B. It evaluates four held-out seeds for each cue. Compare the fraction of
runs with `completion.correct` set to true in `cue-choice-results/results/*.json`
with the mean reward printed by the trainer. This game has deterministic
rewards for each cue-choice pair, so the two values agree when the table and the
evaluation use all four cues with equal weight. Use a fresh output directory
in `eval.yaml` to run a new evaluation without resuming existing results.

To run the integration test from the repository root, use the training Python
environment. You can point it to a specific CLI binary with `PARLANDO_BIN`.

```bash
python -m unittest discover -s parlando-rl/tests -p test_cue_choice.py -v
```
