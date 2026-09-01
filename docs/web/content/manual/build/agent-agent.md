---
title: Run agent-only studies
---

# Run agent-only studies

An agent-only study evaluates two software policies under controlled game configurations, seeds,
seat assignments, and response schedules. It uses the same game and agent interfaces as a live
study, but it does not create participant sessions or open participant intake. Its results are
written to a separate directory.

This mode is appropriate when the unit of analysis is a policy interaction rather than a recruited
participant session. It supports batch evaluation, policy comparison, trajectory inspection, and
the session component of reinforcement learning.

## Run the included comparison

Complete the Python SDK installation in [Attach an automated player](agents-and-voice/), then:

1. Start the included Python policy:

   ```sh
   .venv-agent/bin/python docs/web/examples/great_tree_python_agent.py
   ```

2. In a second terminal, run the checked-in Great Tree comparison:

   ```sh
   cargo run --manifest-path games/great-tree/server/Cargo.toml \
     --bin agent_experiment -- \
     docs/web/examples/great-tree-python-vs-root-bot.yaml
   ```

3. Open the output directory named in the YAML. Confirm that it contains `run.json` and one JSON
   result per expanded plan.
4. Inspect `run.json` for the saved specification and each result for status, policy identities,
   outcome, and the requested trace.
5. Run the same command again. Finalized plans should be skipped rather than duplicated.

Use a new empty output directory when you want a new statistical sample. Do not delete individual
result files and reuse the directory as a way to change the condition; edit the specification and
choose a new directory instead.

## Treat the run specification as the condition

A headless YAML file plays the role that an experiment revision plays for live data. It identifies
the game, names agent configurations, defines a session matrix and schedule, imposes limits, and
selects the output directory. Parlando refuses to mix a changed specification with existing results
in that directory.

A compact evaluation looks like this:

```yaml
schema: parlando-agent-experiment/v1

game:
  id: my-game

agents:
  candidate:
    factory: candidate-policy
    settings: { temperature: 0.2 }
  baseline:
    factory: scripted-baseline

sessions:
  scenarios:
    - name: evaluation
      config: { difficulty: standard }
      seeds: [1, 2, 3]
  seats:
    player_a: candidate
    player_b: baseline
  mirror_roles: true

schedule:
  kind: alternate_after_action
  first: player_a

output:
  directory: results/candidate-vs-baseline
  trace: decisions
```

The names under `agents` are local aliases. Each `factory` must be registered by the game's runner,
its settings must be valid for that policy, and each scenario must be valid game configuration.

## Define what is being compared

The session matrix is a compact factorial design. Scenarios vary the game configuration; seeds vary
deterministic initialization; repetitions request repeated samples; seats determine which policy
occupies `A` and `B`. `mirror_roles: true` adds a reversed seat assignment for each expanded plan.

Use mirrored roles when seat or domain-role asymmetry could confound the policy comparison. Do not
use it mechanically: a policy that implements only one domain role cannot acquire the other role by
changing seats. If game configuration maps seats to domain roles, interpret the seat assignment and
that mapping together.

The runner creates separate session-local agent instances even when both seats select the same
factory. This makes self-play possible without sharing mutable policy state between the two players.

## Make decision opportunities explicit

Agents do not run continuously. The schedule determines which seat receives the next decision
opportunity:

- `alternate_after_action` retains control after a message or rejected action and changes control
  after an accepted action or yield;
- `alternate_every_response` changes control after every response, including messages, rejections,
  and yields.

For most tasks in which an agent may communicate before committing an action,
`alternate_after_action` is the clearer default. `alternate_every_response` is appropriate when
every emitted response consumes a turn. The schedule decides who is asked; `Game::apply_action`
still decides whether the proposed action is legal.

Two consecutive yields end a quiescent session. A combined action and message applies the action
first; the message is delivered only if the action is accepted and does not complete the game. This
ordering prevents a message from claiming that an action occurred when the game rejected it.

## Bound policy failure

A batch needs finite limits even when the game itself normally terminates. Bound callback and
shutdown time, total session time, decisions, accepted actions, messages, and rejected actions.
These limits distinguish a policy failure from an endlessly running experiment.

`execution.concurrency` controls independent sessions, not parallel decisions within one session.
Start at one for a new remote model and increase only after observing resource use and provider
limits. With `fail_fast: false`, one failed plan is finalized while unrelated plans continue. With
`fail_fast: true`, the runner stops on the first failure and executes sequentially.

## Choose evidence deliberately

The trace level controls how much decision evidence accompanies each result:

| Trace | Retained evidence |
| --- | --- |
| `results` | Outcome, failure, policy identities, settings, and counters |
| `decisions` | Results plus responses and action rejections |
| `full` | Decisions plus the acting role's observation and available actions |

Use the least detailed trace that answers the evaluation question. A role-safe observation may
still contain sensitive task or model content, so `full` is not a privacy guarantee.

The output directory contains `run.json` and one JSON file per planned session. Repeating the same
specification resumes the run and skips every finalized result, including failures. Use a new empty
directory for a fresh statistical sample. A changed game identity, game version, or specification
is rejected rather than silently combined with earlier results.

## Attach external policies without changing the design

The `remote_grpc` factory lets a headless run select the same Python service used in a live
human–agent condition:

```yaml
agents:
  python_policy:
    factory: remote_grpc
    settings:
      endpoint: http://127.0.0.1:50051
      config_yaml: |
        policy: evaluation
```

The YAML mapping becomes `Context.settings` in Python. Keep credentials in the Python process. For
remote hosts, the same TLS, hostname allowlist, and transport-token rules apply as in live studies.

{{< callout type="example" title="Run the included attachment example" >}}
Start the manual's Python service, then run its three-session Great Tree comparison:

```sh
.venv-agent/bin/python docs/web/examples/great_tree_python_agent.py
```

```sh
cargo run --manifest-path games/great-tree/server/Cargo.toml \
  --bin agent_experiment -- \
  docs/web/examples/great-tree-python-vs-root-bot.yaml
```

The example exists to show that one external policy can attach to both execution modes. Its
scenario and policies are illustrative; the general design is the run specification above.
{{< /callout >}}

Reinforcement learning adds reward functions, checkpoint-pinned factories, training batches, and
validation cadence to this session model. Those mechanisms are useful only after the ordinary run
specification, schedule, limits, and evidence boundary are well defined.

The following chapter returns to live sessions and adds the communication interfaces through which
two humans or a human and agent exchange language: [Enable typed or spoken dialogue](voice/).
