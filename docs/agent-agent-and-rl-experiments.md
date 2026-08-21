# Run Agent–Agent and Reinforcement-Learning Experiments

Parlando can run two software agents against the same typed `Game` used by live
participants. These headless sessions do not use browsers, WebSockets, the live
database, or dashboard session IDs. Instead, an `ExperimentRunner` reads a YAML
file, creates fresh game and agent instances for each planned session, drives
the agents according to an activation schedule, and writes one immutable JSON
result per session.

Use this runner for batch evaluation, comparisons between policies, and
reinforcement learning. Human–human and human–agent sessions remain
event-driven: humans act whenever they send input. The activation schedule
described here applies only when both players are software agents.

## Run a small evaluation

An agent experiment consists of a small Rust binary and a YAML file. The binary
registers the implementations compiled for your game; the YAML selects among
them and describes the data to run.

Create a binary like this in your game crate:

```rust,ignore
use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use parlando::{ExperimentRunner, GameMetadata};

#[derive(Parser)]
struct Arguments {
    /// Path to a parlando-agent-experiment/v1 YAML file.
    experiment: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    let arguments = Arguments::parse();
    let metadata = GameMetadata {
        id: "my-game".into(),
        name: "My Game".into(),
        version: semver::Version::parse(env!("CARGO_PKG_VERSION"))?,
        build_manifest: serde_json::json!({
            "name": env!("CARGO_PKG_NAME"),
            "version": env!("CARGO_PKG_VERSION"),
        }),
    };

    let summary = ExperimentRunner::new(metadata, MyGameFactory)?
        .agent(CandidateFactory)?
        .agent(BaselineFactory)?
        .run_yaml(arguments.experiment)
        .await?;

    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}
```

`MyGameFactory`, `CandidateFactory`, and `BaselineFactory` are ordinary
`GameFactory` and `AgentFactory` implementations. The stable ID returned by
each agent factory's `Definition` is the value used by YAML. The runner creates
two separate session-local agents even when both seats select the same factory.

The smallest useful evaluation file is:

```yaml
schema: parlando-agent-experiment/v1

game:
  id: my-game

agents:
  candidate:
    factory: candidate
    settings:
      temperature: 0.7
  baseline:
    factory: scripted-baseline

sessions:
  scenarios:
    - name: introductory
      config: { level: introductory }
      seeds: [1, 2, 3]
  seats:
    player_a: candidate
    player_b: baseline
  mirror_roles: true

schedule:
  kind: alternate_after_action
  first: player_a

output:
  directory: results/my-evaluation
  trace: decisions
```

Replace the factory IDs and settings with definitions registered by your
binary. Each scenario's `config` must deserialize as your game's `G::Config`.
The runner validates every game configuration and agent setting before it
starts a session.

This example expands to six sessions: three seeds in the stated seat assignment
and three with the seats reversed. Set `mirror_roles: false` when an agent can
occupy only one role or role reversal is not meaningful. Set `repetitions` on a
scenario when you need repeated samples for each seed:

```yaml
scenarios:
  - name: introductory
    config: { level: introductory }
    seeds: [1, 2, 3]
    repetitions: 5
```

Run the binary with the YAML path:

```bash
cargo run --bin agent-experiment -- experiments/evaluation.yaml
```

## Choose when agents respond

An agent does not run continuously. The session runner calls `Agent::respond`
only when the selected activation schedule gives that role a decision
opportunity. Game completion and hard limits take precedence over the schedule.

Parlando includes two schedules:

- `alternate_every_response` passes control after every response, including a
  message, action, rejected action, or yield.
- `alternate_after_action` keeps control after a message or rejected action,
  and passes control after an accepted action or yield.

Both schedules end a quiescent session after the two agents yield
consecutively. A yield is `Ok(None)` from `Agent::respond`; it is not a message
or game action.

For most turn-based games, use `alternate_after_action`. It lets an agent send
one or more messages before committing an action. Use
`alternate_every_response` when every emitted response should consume a turn.
If neither rule fits, implement `AgentSchedule<G>` in Rust and register it with
`ExperimentRunner::schedule`. YAML should select a compiled policy by name and
small parameters, not encode a scheduling program.

Every action still passes through `Game::apply_action`. The schedule decides
who is asked; it does not decide which actions are legal. Messages never change
game state. For a combined action and message, Parlando applies the action
first and delivers the message only when the action is accepted and does not
complete the game.

## Bound sessions and concurrency

Headless runs must remain finite even when an agent repeatedly yields, sends
messages, or proposes invalid actions. You can override the per-session limits
and cross-session concurrency:

```yaml
limits:
  callback_timeout_seconds: 60
  shutdown_timeout_seconds: 10
  session_timeout_seconds: 900
  decisions: 200
  accepted_actions: 100
  messages: 200
  rejected_actions: 10

execution:
  concurrency: 8
  fail_fast: false
```

These are also the default limits; default concurrency is one. Callbacks are
serialized within a session, while separate sessions may run concurrently.
With `fail_fast: false`, a failed session is finalized and unrelated sessions
continue. With `fail_fast: true`, the runner stops after a failed session and
runs sessions sequentially.

## Inspect results and resume a run

The output directory contains:

```text
results/my-evaluation/
  run.json
  results/
    sha256-<plan-id>.json
```

`run.json` identifies one execution of one normalized YAML specification. Each
session result records its `run_id`, deterministic `plan_id`, scenario, game
seed, both resolved agent identities and roles, settings fingerprints,
completion or failure, counters, elapsed time, and the selected trace.

Choose the least expensive trace that answers your question:

- `results` stores no decision trace;
- `decisions` stores responses and action rejections;
- `full` also stores the acting role's role-safe observation and available
  actions.

Use `full` only when you need model inputs for diagnosis. Role-safe does not
mean non-sensitive: observations and agent messages may still contain study or
participant data, so protect the output directory accordingly.

Running the same YAML against the same output directory resumes the run. Every
finalized plan is skipped, including a failed plan; Parlando does not retry an
entire session automatically. Use a new empty output directory when you want a
new statistical sample. If the YAML, game ID, or game version differs,
Parlando refuses to reuse the directory rather than mixing incompatible
results.

## Supply secrets without writing values to YAML

Agent settings use the same `game.<key>` secret references as live agents. A
headless YAML file maps the unprefixed key to an environment variable:

```yaml
secrets:
  provider_token:
    environment: PROVIDER_TOKEN

agents:
  candidate:
    factory: remote-candidate
    settings:
      endpoint: https://agent.example.org
      token: game.provider_token
```

The runner reads `PROVIDER_TOKEN` at startup. Secret values are delivered only
according to the factory definition's declared purpose and are excluded from
YAML normalization, fingerprints, plans, and results. Library callers may use
`ExperimentRunner::secret("provider_token", value)` instead.

## Add reinforcement learning

RL uses the same sessions and agent lifecycle. The runner owns experiment
cadence: it executes training scenarios, groups trajectories into updates,
switches to the returned checkpoint, and runs held-out validation. The learner
owns policy details: it encodes observations, decodes actions, optimizes the
model, and decides where checkpoints live.

Three additions are required:

1. implement `RewardFunction<G>` for the learning objective;
2. implement `RLAgent<G>` for checkpoint-pinned inference and training; and
3. register both implementations in the experiment binary.

Registration extends the earlier binary by two calls:

```rust,ignore
let summary = ExperimentRunner::new(metadata, MyGameFactory)?
    .agent(BaselineFactory)?
    .rl_agent(MyLearner)?
    .reward(MyReward)?
    .run_yaml(arguments.experiment)
    .await?;
```

Do not also register the learner with `.agent(...)`. An `RLAgent` exposes each
checkpoint as an ordinary `AgentFactory`, so its single factory ID covers both
session inference and training.

### Define rewards

Rewards are separate from `Game` because one game may support several learning
objectives, while evaluation-only games need none. A reward function receives
the authoritative state transition after an accepted action and returns one
number for each fixed role:

```rust,ignore
use parlando::{PlayerRole, RewardFunction, RoleRewards};

struct WinLossReward;

impl RewardFunction<MyGame> for WinLossReward {
    fn id(&self) -> &'static str { "win_loss" }
    fn version(&self) -> &'static str { "1" }

    fn validate_parameters(&self, parameters: &serde_json::Value) -> anyhow::Result<()> {
        // Parse and validate any reward-specific YAML values here.
        Ok(())
    }

    fn rewards(
        &self,
        _before: &MyState,
        _after: &MyState,
        _actor: PlayerRole,
        _action: &MyAction,
        completion: Option<&MyCompletion>,
        _parameters: &serde_json::Value,
    ) -> RoleRewards {
        match completion.and_then(|value| value.winner) {
            Some(PlayerRole::A) => RoleRewards { player_a: 1.0, player_b: -1.0 },
            Some(PlayerRole::B) => RoleRewards { player_a: -1.0, player_b: 1.0 },
            None => RoleRewards { player_a: 0.0, player_b: 0.0 },
        }
    }
}
```

Only the numeric `RoleRewards` enter the trajectory. Authoritative game state
does not cross the learner boundary.

### Expose checkpoint-pinned inference and training

`RLAgent<G>` connects checkpoint management to ordinary agents:

```rust,ignore
use parlando::{
    agent::Factory as AgentFactory, CheckpointId, RLAgent, RLTrainingContext,
    TrainingBatch,
};

#[async_trait::async_trait]
impl RLAgent<MyGame> for MyLearner {
    fn factory_id(&self) -> &str {
        "my_learner"
    }

    fn resolve_checkpoint(&self, reference: &serde_json::Value)
        -> anyhow::Result<CheckpointId>
    {
        let value = reference
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("checkpoint must be a string"))?;
        CheckpointId::new(value)
    }

    fn factory(&self, checkpoint: &CheckpointId)
        -> anyhow::Result<std::sync::Arc<dyn AgentFactory<MyGame>>>
    {
        Ok(std::sync::Arc::new(MyCheckpointFactory::new(checkpoint.clone())))
    }

    async fn train(
        &mut self,
        context: &RLTrainingContext,
        base: &CheckpointId,
        batch: TrainingBatch,
    ) -> anyhow::Result<CheckpointId> {
        // Decode batch.steps, update from `base`, and publish a new checkpoint.
        train_policy(context, base, batch).await
    }
}
```

`factory(checkpoint)` must return a normal factory pinned to an immutable
checkpoint. Its agents receive typed, role-safe `G::Observation` values and
optional available actions, then return typed `G::Action` values. Parlando does
not define a game-independent observation vector; encoding belongs in your
learner.

`TrainingBatch` contains ordered action transitions for the named learner. Each
step includes the role-safe observation, available actions, chosen action,
acceptance or rejection, per-role reward for accepted actions, next role-safe
observation, terminal flag, scenario, role, checkpoint, and stable run/plan/
decision identity. Version 1 captures action decisions only; it does not assign
RL steps or credit to messages and yields.

`RLTrainingContext` contains the learner's normalized YAML settings and
factory-purpose secrets. It is supplied directly to `train`; do not configure
training through side effects of session-agent construction.

Checkpoint IDs are opaque to Parlando. They may refer to memory, files, a
database, or a remote learner. They must be non-empty and semantically
immutable during the run. `train` must treat `batch.update_id` idempotently:
the same update can be presented again if a learner update succeeds before its
checkpoint record is finalized. Physical checkpoint storage and save cadence
remain learner responsibilities.

After each successful update, Parlando atomically writes a numbered checkpoint
record under the experiment output directory. This record links the update ID,
base checkpoint, and returned checkpoint for resume; it is not the model
checkpoint itself. If `RLAgent` delegates inference or training to another
process, start that service before the Rust experiment runner.

### Configure training and validation

An RL experiment replaces or complements `sessions` with `training` and
optional `validation` sections:

```yaml
schema: parlando-agent-experiment/v1

game:
  id: my-game

agents:
  learner:
    factory: my_learner
    checkpoint: initial
    settings:
      learning_rate: 0.0001
      checkpoint_directory: artifacts/checkpoints
      save_every_checkpoints: 5
  baseline:
    factory: scripted-baseline

training:
  learner: learner
  scenarios:
    - name: train-easy
      config: { level: easy }
      seeds: [1, 2, 3, 4]
    - name: train-hard
      config: { level: hard }
      seeds: [10, 11, 12, 13]
  seats:
    player_a: learner
    player_b: baseline
  mirror_roles: false
  epochs: 20
  checkpoint_every_epochs: 2
  reward:
    kind: win_loss
    parameters: {}

validation:
  scenarios:
    - name: held-out
      config: { level: hard }
      seeds: [1001, 1002, 1003, 1004]
  seats:
    player_a: learner
    player_b: baseline
  mirror_roles: false
  every_checkpoints: 2

schedule:
  kind: alternate_after_action
  first: player_a

execution:
  concurrency: 8
  fail_fast: false

output:
  directory: results/my-training-run
  trace: results
```

One epoch is one complete deterministic sweep over the expanded training
scenarios. In this example, every two epochs produce one learner update, and
every second returned checkpoint triggers one validation sweep. Training and
validation may assign different opponents or seats, but the named learner must
occupy at least one seat in both sections. `epochs` must be divisible by
`checkpoint_every_epochs`; all cadences must be positive.

Validation results are keyed by the checkpoint used for inference and do not
enter subsequent training batches. If training fails, the loop stops because
there is no valid next checkpoint; already finalized session results remain
available for diagnosis and resume.

For self-play, assign the same named learner to both training seats. The runner
creates two agents pinned to the same checkpoint and captures action
transitions from both roles. Version 1 still performs one update for one named
learner; independently updating two learners requires a separate training
procedure.

## Use the cue-choice implementation as a reference

The [cue-choice RL experiment](../experiments/cue-choice-rl/README.md) is a
complete local example. Its source shows:

- a typed game and role-safe observations in
  [`server/src/game.rs`](../experiments/cue-choice-rl/server/src/game.rs);
- a scripted agent and reward function in
  [`server/src/agents.rs`](../experiments/cue-choice-rl/server/src/agents.rs);
- registration of Parlando's reusable `RemoteAgent` in
  [`server/src/bin/agent_experiment.rs`](../experiments/cue-choice-rl/server/src/bin/agent_experiment.rs);
- a combined inference and learner service in
  [`python/src/cue_choice_rl/server.py`](../experiments/cue-choice-rl/python/src/cue_choice_rl/server.py);
  and
- the complete training and validation file in
  [`experiment.yaml`](../experiments/cue-choice-rl/experiment.yaml).

The example uses a Python Qwen/TorchRL learner. The Parlando Python SDK hides
the protobuf service implementations, so experiment code receives opaque
configuration and JSON-shaped trajectory steps. Python is not required by the
runner; an in-process Rust learner can still implement `RLAgent<G>` directly.
