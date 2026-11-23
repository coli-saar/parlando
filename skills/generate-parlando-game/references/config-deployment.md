# Package and deploy a game

The project `game.yaml` declares game identity and build inputs.
`parlando build` creates the deployable ZIP at `dist/<game-id>-<game-version>.zip`.
Its manifest points to the exact component and optional built participant
assets. Set `PACKAGE` to the path printed by the build. Run `parlando check --package "$PACKAGE"` before
handing off or uploading that ZIP.

For a [local pilot](https://coli-saar.github.io/parlando/manual/start/getting-started/#start-a-local-server),
start a server with `parlando serve --store DIRECTORY`, deploy the ZIP in
**Games**. Open its game ID link and use the game
dashboard to configure the experiment and start a local preview. Study settings
such as pairing, consent, intake, communication, and agent selection belong to
the experiment. Game-owned parameters go in the game's typed configuration.
Store secrets outside packages, browser assets, and experiment configuration.

For an authorized deployment, use the shared dashboard or
`parlando deploy --host <origin> --package "$PACKAGE" --json`.
Sign in once with `parlando login --server SERVER_URL` and choose **Authorize CLI** in the browser. The saved credential can deploy any game on that server. Record the returned revision with
the study data. A new upload replaces the current package if the game's
experiments and sessions have stopped. Follow [Deploy to a shared host](https://coli-saar.github.io/parlando/manual/deploy/)
for credential setup and operator commands.

For a local WASM policy, set `GAME_PACKAGE` to the built game ZIP path.
Pin it with `parlando agent set-game "$GAME_PACKAGE" --manifest agent.yaml`,
then build a separate agent ZIP with `parlando build --manifest agent.yaml`.
Set `AGENT_PACKAGE` to the agent ZIP path and run
`parlando check --package "$AGENT_PACKAGE"`. Upload the game ZIP first.
Use the same account to upload the agent ZIP to the same host while the
game's experiments are stopped and its sessions have finished:

```sh
parlando deploy --host "$HOST" --package "$GAME_PACKAGE" --json
parlando deploy --host "$HOST" --package "$AGENT_PACKAGE" --json
```

Record both returned revisions. In the game dashboard, choose the installed
factory in the experiment's configuration and verify the pairing in a local
preview. Game updates retain installed agents and validate their payload layouts. For an external service, choose `remote_grpc` and configure
its endpoint in the experiment.
