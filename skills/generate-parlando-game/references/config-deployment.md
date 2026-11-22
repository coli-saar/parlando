# Package and deploy a game

The project `game.yaml` declares game identity and build inputs.
`parlando build` creates the deployable ZIP at `dist/<game-id>-<game-version>.zip`.
Its manifest points to the exact component and optional built participant
assets. Set `PACKAGE` to the path printed by the build. Run `parlando check --package "$PACKAGE"` before
handing off or uploading that ZIP.

For a [local pilot](https://coli-saar.github.io/parlando/manual/start/getting-started/#start-a-local-server),
start a server with `parlando serve --store DIRECTORY`, deploy the ZIP in
**Games**, and activate the game. Open its game ID link and use the game
dashboard to configure the experiment and start a local preview. Study settings
such as pairing, consent, intake, communication, and agent selection belong to
the experiment. Game-owned parameters go in the game's typed configuration.
Store secrets outside packages, browser assets, and experiment configuration.

For an authorized deployment, use the shared dashboard or
`parlando deploy --host <origin> --package "$PACKAGE" --json`.
Sign in once with `parlando login --server SERVER_URL` and choose **Authorize CLI** in the browser. The saved credential can deploy any game on that server. Record the returned revision with
the study data. A new upload replaces the current package if the game's
experiments and sessions have stopped. Game activation through the CLI requires
host administrator credentials. Follow [Deploy to a shared host](https://coli-saar.github.io/parlando/manual/deploy/)
for credential setup and operator commands.
