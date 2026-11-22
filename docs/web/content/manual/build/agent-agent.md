---
title: Run an agent-only evaluation
---

# Run an agent-only evaluation

Use `parlando run` to play a game with an agent in each seat. It loads the same WASM component as a live experiment. The evaluation file controls the role assignments, starting configurations, seeds, and output directory.

Build Beacon Relay and start the [Beacon agent service](agents-and-voice/) locally. Save this as `evaluation.yaml` in the game project.

```yaml
schema: parlando-agent-experiment/v1
game:
  id: beacon
agents:
  policy:
    factory: remote_grpc
    settings:
      endpoint: http://127.0.0.1:50051
sessions:
  scenarios:
    - name: baseline
      config: {}
      seeds: [7, 8]
  seats:
    player_a: policy
    player_b: policy
schedule:
  kind: alternate_every_response
  first: player_a
output:
  directory: results/beacon-baseline
  trace: decisions
```

```sh
parlando build
parlando run evaluation.yaml --package dist/beacon-0.1.0.zip --json
```

Each session has separate agent instances, even when both seats use the same service. The command reports counts for completed, failed and skipped sessions. Review the output files and investigate failed sessions before aggregating results.

Rerun the same specification and component with the same output directory to resume an interrupted evaluation. Completed result files stay in place and finalized failures remain recorded. Use a new directory for another sample or any changed component or specification.

## Choose Wasm metering for agent–agent runs

By default, each call into the game component gets 50 million Wasmtime fuel units. If the game exhausts that budget, the call fails. For high-volume evaluations of a trusted game, add this block to the evaluation YAML to run without fuel accounting:

```yaml
execution:
  wasm_metering: none
```

This can improve throughput when both agents act quickly and much of the run is spent executing game code. It does not speed up agent inference or network calls. Compare completed sessions per second on your game with the same agents, seeds and concurrency, using a separate output directory for each mode.

With `none`, a game that loops inside a call can hold a worker indefinitely. The session timeout and action limits cannot interrupt that call. Guest memory limits still apply. Set `wasm_metering: fuel` or omit the field to restore the per-call budget.
