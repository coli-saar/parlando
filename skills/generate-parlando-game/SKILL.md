---
name: generate-parlando-game
description: Implement, port, or update a Parlando game from supplied rules using the Parlando CLI, Rust or Python Game SDK, and participant client. Use for technical game creation, optional policy authoring, and packaging from an existing specification.
---

# Generate a Parlando game

Implement the supplied game specification. Ask a focused question only when a missing rule or interface detail blocks implementation. Leave decisions about goals, scoring, difficulty, and player experience with the author.

## Start from the installed CLI

For a new game, default to Rust and run `parlando new <game-id> --lang rust`. Use `--lang python` when the user requests Python. For an existing project, inspect its `game.yaml`, dependencies, and scenarios, and keep its implementation language unless the user requests a port.

When porting a 0.6.0 game to 0.7.0, follow the [migration guide](https://github.com/coli-saar/parlando/blob/main/docs/migrating-0.6.0-to-0.7.0.md) for native mechanics, browser dependencies, and existing study data. Use the starter and the documentation for the project's Parlando release to establish the SDK API and file layout. Keep the scaffold's matching SDK and browser dependencies. The [experimenter manual](https://coli-saar.github.io/parlando/manual/) covers tool installation and package conventions.

## Implement the mechanics and browser contract

- Use the public Game SDK to implement session-owned mechanics for roles A and B. Keep private state in the component, derive random behavior from the supplied seed, and reject invalid actions before mutating state or consuming randomness. Actions change task state. Messages use the participant runtime.
- Define a view for each role. Expose only that role's authorized information and keep the shared completion safe for both roles. See [component mechanics](references/server-adapter.md) for Rust and Python integration and recovery.
- With the starter's React client, render the view through `ParticipantApp` and send actions through `snapshot.act`. Let the SDK handle participant entry, connections, and terminal handling. The experiment selects typed chat or speech. Keep the default communication widget, or place one `CommunicationWidget` inside the game and style it using the package's CSS hooks. See [browser client integration](references/browser-client.md) for the placement and styling contract.
- Show Prolific-specific participant copy only for a seat admitted through Prolific. In game UI, use `snapshot.recruitmentSource === "prolific"`. Let `ParticipantApp` render return codes and links from the server-issued handoff. In the packaged game, check both a direct waiting-room exit and a Prolific admission. See the [browser client reference](references/browser-client.md).
- Keep study settings and secrets in the host's configuration. Use typed game configuration for the game's own parameters. Read [deployment](references/config-deployment.md) when the task includes upload or hosting.

## Add a policy when requested

Use the same Rust SDK to write a typed policy against the game's compact
action, role-private view, and completion. Build the game ZIP first and
pin it with `parlando agent new <agent-id> --game "$PACKAGE"` for a new
standalone policy or `parlando agent set-game "$PACKAGE"` for an existing
agent manifest. Build the separate agent ZIP and exercise it against the
pinned game in a seeded experiment.
Use the built-in remote gRPC adapter for a separately hosted policy. Follow
[agent integration](references/agents.md) for implementation and validation.

## Validate the package

From the game project, run:

```sh
parlando test
PACKAGE=$(parlando build --json | python3 -c 'import json, sys; print(json.load(sys.stdin)["result"]["package"])')
parlando check --package "$PACKAGE"
```

Update the generated scenarios to exercise relevant accepted and rejected actions, both role views, the terminal outcome, and recovery where appropriate. For a local pilot, [start a server, deploy the game, and start a local preview](https://coli-saar.github.io/parlando/manual/start/getting-started/#start-a-local-server). Use the client tarball from the same candidate when working from an unreleased Parlando checkout. A game version bump alone keeps the dependency resolved by npm. Open the packaged participant landing page with the game theme applied. For a speech pilot, check the spacing of the voice status and controls before permission, then prepare a microphone and confirm the shared level meter responds when you speak, both on entry and while the first participant waits for a partner. Preserve the SDK meter and its CSS hooks when theming these screens. For a human–agent speech experiment, pilot that pairing with its configured startup gates. Check the selected microphone name, reload and prepare it again while waiting, then verify activation and the first legal action. For typed entry, confirm the voice controls are absent. If the participant interface changed, verify that actions are disabled outside an active interaction and that the intended view and completion render. Use two independent browser profiles for a two-human pilot.

Report the package path, the checks that ran, and the local serve command. Include deployment results or agent commands when those are part of the request.
