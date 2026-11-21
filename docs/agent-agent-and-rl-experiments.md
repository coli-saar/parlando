# Run Agent–Agent and Reinforcement-Learning Experiments

Parlando can run two software agents against the same WebAssembly game component used by live
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

A headless experiment uses a built game component and a YAML file. If the
experiment uses remote policies for evaluation, run it with `parlando run` from
your game project. To register in-process policies or a learner, write a small
Rust launcher that loads the component and registers the factories named in the
YAML.

```rust,ignore
use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use parlando::{component::LoadedGame, ExperimentRunner, GameMetadata};

#[derive(Parser)]
struct Arguments {
    /// Path to a built game component.
    #[arg(long)]
    component: PathBuf,
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

    let game = LoadedGame::load(&arguments.component)?;
    let summary = ExperimentRunner::new(metadata, game)?
        .agent(CandidateFactory)?
        .agent(BaselineFactory)?
        .run_yaml(arguments.experiment)
        .await?;

    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}
```

`CandidateFactory` and `BaselineFactory` implement `agent::Factory`. The stable ID returned by
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
binary. The runner validates every scenario’s `config` against the component’s
declared configuration schema and validates the agent settings before it starts
a session. The component can apply additional constraints during session
initialization.

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
If neither rule fits, implement `AgentSchedule` in Rust and register it with
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

`run.json` identifies the normalized YAML specification and the SHA-256 digest
of the loaded component. Each session result records its `run_id`, deterministic `plan_id`, scenario, game
seed, both resolved agent identities and roles, settings fingerprints,
completion or failure, counters, elapsed time, and the selected trace.

Choose the least expensive trace that answers your question:

- `results` stores no decision trace;
- `decisions` stores responses and action rejections;
- `full` also stores the acting role’s observation. The observation contains
  a legal-action catalogue if the game publishes one in its view.

Use `full` only when you need model inputs for diagnosis. Role-safe does not
mean non-sensitive: observations and agent messages may still contain study or
participant data, so protect the output directory accordingly.

Running the same YAML against the same output directory resumes the run. Every
finalized plan is skipped, including a failed plan; Parlando does not retry an
entire session automatically. Use a new empty output directory when you want a
new statistical sample. Resuming requires the same YAML, game ID, game version,
and component digest. A run manifest from a native game has no component
digest and requires a new output directory.

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

This mechanism applies to factories that declare secret-reference fields. The standard
`remote_grpc` factory declares none: keep provider credentials in the remote process and use
`PARLANDO_REMOTE_AGENT_TOKEN` only for deployment-level transport authentication.

## Add reinforcement learning

RL uses the same sessions and agent lifecycle. The runner owns experiment
cadence: it executes training scenarios, groups trajectories into updates,
switches to the returned checkpoint, and runs held-out validation. The learner
owns policy details: it encodes observations, decodes actions, optimizes the
model, and decides where checkpoints live.

Three additions are required:

1. implement `RewardFunction` for the learning objective;
2. implement `RLAgent` for checkpoint-pinned inference and training; and
3. register both implementations in the experiment binary.

Registration extends the earlier binary by two calls:

```rust,ignore
let summary = ExperimentRunner::new(metadata, game)?
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

A single component can support several learning objectives. A reward function
receives the component frames before and after an accepted action and returns one
number for each fixed role. It can read both role views, transition metadata, and
the shared completion when present. Implement `configure` to parse and validate the
YAML parameters once during run setup. The same configured reward serves every
training session in the run.

```rust,ignore
use parlando::{component::{ComponentValue, GameFrame}, PlayerRole, RewardFunction, RoleRewards};

#[derive(serde::Deserialize)]
struct Completion {
    winner: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct WinLossReward {
    win: f64,
}

impl RewardFunction for WinLossReward {
    fn id(&self) -> &'static str { "win_loss" }
    fn version(&self) -> &'static str { "1" }

    fn configure(&self, parameters: &serde_json::Value) -> anyhow::Result<Self> {
        let reward: Self = serde_json::from_value(parameters.clone())?;
        anyhow::ensure!(reward.win.is_finite() && reward.win > 0.0, "win must be positive");
        Ok(reward)
    }

    fn rewards(
        &self,
        _before: &GameFrame,
        _after: &GameFrame,
        _actor: PlayerRole,
        _action: &ComponentValue,
        completion: Option<&ComponentValue>,
    ) -> RoleRewards {
        let winner = completion
            .and_then(|value| value.decode::<Completion>().ok())
            .and_then(|value| value.winner);
        match winner.as_deref() {
            Some("A") => RoleRewards { player_a: self.win, player_b: -self.win },
            Some("B") => RoleRewards { player_a: -self.win, player_b: self.win },
            _ => RoleRewards { player_a: 0.0, player_b: 0.0 },
        }
    }
}
```

Register the implementation with `runner.reward(WinLossReward { win: 1.0 })?`
and select it under `training` in the experiment YAML.

```yaml
training:
  reward:
    kind: win_loss
    parameters:
      win: 1.0
```

Only the numeric `RoleRewards` enter the trajectory. Authoritative game state
does not cross the learner boundary.

### Expose checkpoint-pinned inference and training

`RLAgent` connects checkpoint management to ordinary agents:

```rust,ignore
use parlando::{
    agent::Factory as AgentFactory, CheckpointId, RLAgent, RLTrainingContext,
    TrainingBatch,
};

#[async_trait::async_trait]
impl RLAgent for MyLearner {
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
        -> anyhow::Result<std::sync::Arc<dyn AgentFactory>>
    {
        Ok(std::sync::Arc::new(MyCheckpointFactory::new(checkpoint.clone())))
    }

    async fn train(
        &mut self,
        context: &RLTrainingContext,
        base: &CheckpointId,
        batch: TrainingBatch,
    ) -> anyhow::Result<CheckpointId> {
        // Update from typed batch.steps and publish a new checkpoint.
        train_policy(context, base, batch).await
    }
}
```

`factory(checkpoint)` returns an agent factory pinned to an immutable
checkpoint. Each agent receives its role-specific view and returns actions as
`ComponentValue`. A component can publish legal moves in the view. The learner
defines how to encode that view for its model.

On resume, Parlando reloads finalized learner steps from the JSON result files
into `TrainingBatch`. Each step includes the role-safe observation, chosen action,
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

The accompanying [cue–choice Qwen study](cue-choice-qwen-rl-paper/main.pdf)
documents the experimental question, learning method, validation design, and
results. Read it when you need a research-level account of how the components
below form a reproducible Parlando experiment.

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
runner; an in-process Rust learner can still implement `RLAgent` directly.
