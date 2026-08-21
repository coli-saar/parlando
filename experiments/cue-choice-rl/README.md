# Cue Choice RL experiment

This experiment is a small end-to-end test of Parlando's agent-agent learning
path. It is not intended to benchmark language-model reasoning. Its purpose is
to make each infrastructure boundary observable: two-agent session execution,
role-safe observations, typed actions, reward calculation, trajectory transfer,
learner updates, opaque checkpoints, held-out validation, and resume.

## Decision

Use Python for learning, with TorchRL 0.13.3,
Hugging Face Transformers, PEFT LoRA, and
[`Qwen/Qwen2.5-0.5B-Instruct`](https://huggingface.co/Qwen/Qwen2.5-0.5B-Instruct).
The model has 0.49 billion parameters, an Apache-2.0 license, a standard
Transformers implementation, and an instruction-tuned checkpoint. It is small
enough for a single modest GPU while still exercising a real pretrained LLM.

The Rust ecosystem is capable of model training, but it is not the shortest
path for this experiment. [Burn 0.21](https://github.com/Tracel-AI/burn/releases/tag/v0.21.0)
describes its reinforcement-learning support as early and off-policy, whereas
this design uses on-policy PPO.
[Candle](https://github.com/huggingface/candle) supports model training and Qwen
inference, but combining Qwen fine-tuning, LoRA, a value head, and PPO would
require substantially more experiment-specific machinery. Python therefore
reduces implementation risk without moving game state or scheduling out of
Rust. The Python process implements the learner; Parlando remains the
authoritative environment and experiment coordinator.

Use TorchRL's generalized-advantage implementation with a small explicit
clipped-PPO loss rather than making TorchRL own the environment or rollout
loop. Transformers loads the Qwen backbone, and PEFT adds trainable LoRA
adapters.

## Run on macOS

Python 3.11 or newer and current Xcode command-line tools are required. Create
an isolated environment from this directory and install the local Parlando SDK:

```bash
cd experiments/cue-choice-rl/python
python3 -m venv .venv
source .venv/bin/activate
export HF_HOME="$PWD/.hf-cache"
python -m pip install -e ../../../rust-server/python/parlando-agent-sdk
python -m pip install -e '.[qwen,test]'
python -m cue_choice_rl.generate_protos
```

Start the preloaded learner in one terminal. Apple Silicon uses MPS; use
`--device cpu` on an Intel Mac or when MPS is unavailable:

```bash
cue-choice-learner --backend qwen --device auto
```

Then run the Rust coordinator from the repository root:

```bash
cargo run --manifest-path experiments/cue-choice-rl/server/Cargo.toml -- \
  experiments/cue-choice-rl/experiment.yaml
```

For a quick protocol test without PyTorch or a model download, install only
`-e '.[test]'`, start `cue-choice-learner --backend random`, and use the same
Rust command. The random backend exercises sessions, trajectories, update
idempotency, checkpoints, validation, and resume, but deliberately does not
learn.

## Required runner correction

The former `RLAgent::train(base, batch)` contract did not receive the named
learner's normalized YAML settings or resolved factory secrets. That is enough
for the in-process mock, but not for this experiment: the training method needs
the Python endpoint, hyperparameters, checkpoint directory, and optional
transport credential from the same agent definition used for inference.

The runner now supplies one small context parameter:

```rust,ignore
pub struct RLTrainingContext {
    pub settings: serde_json::Value,
    pub factory_secrets: SecretValues,
}

async fn train(
    &mut self,
    context: &RLTrainingContext,
    base: &CheckpointId,
    batch: TrainingBatch,
) -> Result<CheckpointId>;
```

`ExperimentRunner` already normalizes settings and resolves factory secrets
while it resolves the learner's ordinary factory. It should retain those values
once and pass them to every update. Do not make `AgentFactory::identity` or
`create` mutate shared learner configuration as a side effect; that would make
training depend on whether and in what order sessions happened to construct
agents. Agent-instance secrets are unnecessary for training and should not be
included in this context.

## The game

`cue-choice` is a two-player, two-action game. Player B is a scripted dealer and
player A is the learner.

1. The dealer receives `ReadyToDeal` and submits `Deal`.
2. The learner receives a private cue and a trial nonce, then submits one of
   four `Choose` actions.
3. The game completes immediately and reports whether the choice was correct.

The compiled level catalogue defines an arbitrary mapping that is not included
in the learner's observation:

| Level and visible cue | Correct choice |
| --- | --- |
| `dax` | `oak` |
| `wug` | `pine` |
| `kiki` | `birch` |
| `zorp` | `elm` |

The game seed produces an irrelevant `trial_nonce`. Training and validation use
disjoint seeds, so validation checks whether the policy learned the cue mapping
rather than memorizing complete serialized observations. This is not a test of
out-of-distribution generalization: both phases use the same four levels.

The state machine is:

```text
AwaitingDeal -- Deal by B --> Choosing -- Choose by A --> Complete
```

Actions submitted in the wrong phase or by the wrong role are rejected. The
learner always receives the four typed choices as `available_actions`; the
dealer receives only `Deal`. The schedule starts with player B and alternates
after each accepted action. Because the learner's action completes the game,
neither agent needs to yield or send messages.

## Reward

Register `cue_choice.correctness` as a `RewardFunction<CueChoiceGame>`:

```text
dealer Deal:       player A =  0, player B = 0
correct Choose:    player A = +1, player B = 0
incorrect Choose:  player A = -1, player B = 0
```

Player B is an environment actor rather than an opponent, so its reward is
zero. The reward function reads the authoritative completion after `Choose`,
but only the two numeric rewards enter the trajectory.

## Policy and PPO update

The Python learner owns all encoding. It serializes the learner's role-safe
observation as canonical JSON inside a short chat prompt and runs the frozen
Qwen backbone with LoRA adapters on the attention projections. A trainable
linear policy head maps the final hidden state to four fixed semantic choices;
a trainable scalar head estimates value. Invalid choices are masked before
constructing the categorical distribution. The selected semantic choice is
decoded into the corresponding typed `Choose` action.

Both heads start with zero weights and biases. The initial policy is therefore
uniform over the four choices rather than being dominated by an arbitrary
random logit before PPO has observed any reward.

This fixed action head is deliberate. Free-form JSON generation would mostly
test parsing and invalid-output recovery. The experiment instead isolates
whether reward-bearing Parlando trajectories can update a real LLM-backed
policy and produce a usable subsequent checkpoint.

Each episode contains one learner transition, so return and advantage
calculation are unambiguous. For every update batch:

1. load the immutable base checkpoint named by `TrainingBatch`;
2. reconstruct action indices from the selected typed actions;
3. recompute old action log-probabilities and values under that checkpoint;
4. set the return to the learner's terminal reward;
5. run four minibatch epochs of clipped PPO; and
6. publish a new immutable in-memory checkpoint ID.

Initial parameters should be conservative: LoRA rank 8, learning rate `1e-4`,
PPO clip epsilon `0.2`, entropy coefficient `0.01`, value coefficient `0.5`,
and minibatches of 32 transitions. These are smoke-test defaults, not tuned
hyperparameters. Record loss, entropy, approximate KL, clip fraction, and
training-batch reward mean in the update response or learner logs.

The learner keeps every checkpoint needed by the active run in memory and saves
every third checkpoint to disk. Disk persistence is controlled by learner
settings and is not represented as an ExperimentRunner cadence. A checkpoint ID
has the form `cue-qwen25/update-000003`; it is an opaque identifier to Rust.

## Process boundary

The Python process hosts two gRPC services on the same local endpoint:

- the existing `parlando.agent.v4.AgentService` performs checkpoint-pinned
  inference for each session-local agent;
- Parlando's `parlando.rl.v1.LearnerService` applies training batches.

Parlando's non-generic Rust `RemoteAgent` implements both the ordinary factory
and learner traits for every serializable game. It sends the checkpoint in a
dedicated request field and forwards only `settings.config` as opaque data.
The Python SDK owns both gRPC adapters. Cue-choice therefore implements no Rust
or protobuf transport code: its Python classes receive ordinary dictionaries,
choose actions, and train the model. The Python server treats `update_id` as an
idempotency key and returns the previously created checkpoint on replay.

The Python server preloads the base model and initial checkpoint before it
starts listening. Session-local Python agents retain only their latest
observation and a reference to the immutable shared policy, so eight concurrent
sessions do not load eight copies of Qwen. Training constructs a new adapter and
heads without mutating the base checkpoint. A lock excludes checkpoint
publication from inference lookup, although the runner's phase ordering already
prevents training and validation sessions from overlapping.

For idempotency, the service stores the hash and response for every `update_id`.
Repeating an identical request returns the stored response. Reusing an update ID
with a different base checkpoint or trajectory batch fails with
`FAILED_PRECONDITION`; it must never apply a second update under the same ID.

This adapter is experiment infrastructure, not an `RlAdapter`: it transports
already role-safe observations and typed actions and does not define their
meaning. Encoding remains entirely inside the Python learner.

Version 1 may be loopback-only and may reject non-local endpoints. That keeps
authentication and artifact distribution out of the first test. The existing
remote-agent security rules should be reused before supporting a remote learner.

## Experiment file

[`experiment.yaml`](experiment.yaml) contains four training levels, 16 training
seeds per level, and eight disjoint validation seeds per level. Two epochs are
accumulated into each learner update. Twelve epochs therefore produce six
checkpoints and six validation rounds; the learner persists checkpoints 3 and 6
to disk.

One expanded training session plan is conceptually:

```yaml
phase: training
phase_index: 1
scenario: dax
game_seed: 1
seats:
  player_a:
    agent: learner
    checkpoint: initial
  player_b:
    agent: dealer
schedule:
  kind: alternate_every_response
  first: player_b
```

`plan_id` is derived from these coordinates and the normalized configuration;
it is not written into the YAML. On resume, Parlando skips finalized session
plans, reloads their trajectories, and uses atomic checkpoint records to avoid
repeating completed PPO updates.

## Directory and implementation slices

The implementation remains isolated here except for the generic Python helper
which registers an agent service on an existing gRPC server:

```text
experiments/cue-choice-rl/
  README.md
  experiment.yaml
  proto/parlando_rl_v1.proto
  server/                       # Rust game, dealer, reward, RL adapter, CLI
  python/                       # TorchRL/Qwen learner service
  tests/                        # end-to-end smoke and resume tests
```

It is organized in these slices:

1. Add `RLTrainingContext` to the generic runner and test normalized settings
   and factory-secret delivery with the in-process mock learner.
2. Add the Rust game, dealer, reward, and deterministic game tests. Run the YAML
   first with a mock in-process learner.
3. Add the learner gRPC protocol and a Python uniform-random implementation.
   Verify inference, training idempotency, and checkpoint selection without
   loading Qwen.
4. Replace the random policy with Qwen, LoRA, policy/value heads, and TorchRL's
   PPO loss. Keep the wire protocol unchanged.
5. Add a slow opt-in end-to-end test which performs two updates, restarts the
   Rust runner, and verifies that finalized updates are not repeated.

## Acceptance criteria

The required infrastructure result is binary and should not depend on successful
RL tuning:

- all planned training and validation sessions finalize;
- each training session contains exactly one learner trajectory step and no
  authoritative state;
- six update IDs produce six distinct checkpoint IDs;
- validation result provenance names the checkpoint it evaluated;
- checkpoint records 1 through 6 are finalized atomically;
- repeating the command causes no new session callbacks or learner updates; and
- a deliberate duplicate `TrainRequest` returns the same checkpoint ID.

Learning quality is a secondary diagnostic. The initial policy should be near
the 25% random-choice baseline. A useful first target is at least 80% mean
validation accuracy over the final two checkpoints, but failure to reach that
threshold is an optimization failure, not an infrastructure failure. Report the
full checkpoint-by-checkpoint validation curve so a transient final sample does
not determine the conclusion.

## Known limitations

- This is a contextual bandit, not a long-horizon dialogue policy.
- The fixed four-choice policy head does not exercise free-form LLM generation.
- Validation changes the nonce but not the cue-to-choice mapping.
- The current Parlando trajectory omits rollout log-probabilities and values, so
  the learner recomputes them from the immutable base checkpoint.
- In-memory checkpoints cannot survive Python-process loss. Disk checkpoint 3
  or 6 can seed a new run, but resuming between those points requires the same
  learner process. A production transport must persist every checkpoint needed
  by an active resumable run or define explicit recovery behavior.
