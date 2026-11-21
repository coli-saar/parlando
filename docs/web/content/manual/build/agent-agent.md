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
parlando run evaluation.yaml --manifest dist/parlando/game.yaml --json
```

Each session has separate agent instances, even when both seats use the same service. The command reports counts for completed, failed and skipped sessions. Review the output files and investigate failed sessions before aggregating results.

Rerun the same specification and component with the same output directory to resume an interrupted evaluation. Completed result files stay in place and finalized failures remain recorded. Use a new directory for another sample or any changed component or specification.
