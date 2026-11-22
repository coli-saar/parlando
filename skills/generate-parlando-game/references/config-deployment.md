# Package and deploy a game

The project `game.yaml` declares game identity and build inputs.
`parlando build` creates the deployable ZIP at `dist/<game-id>-<game-version>.zip`.
Its manifest points to the exact component and optional built participant
assets. Set `PACKAGE` to the path printed by the build. Run `parlando check --package "$PACKAGE"` before
handing off or uploading that ZIP.

For a [local pilot](https://coli-saar.github.io/parlando/manual/start/getting-started/#start-a-local-server),
start a server with `parlando serve --store DIRECTORY`, deploy the package,
pin its revision with `parlando select`, and open testing intake with
`parlando intake`. Study settings such as pairing,
consent, intake, communication, and agent selection belong to the experiment.
Game-owned parameters go in the game's typed configuration. Store secrets
outside packages, browser assets, and experiment configuration.

For an authorized deployment, use the shared dashboard or
`parlando deploy --host <origin> --package "$PACKAGE" --json`.
The CLI uses a game-scoped upload credential. Record the returned revision.
A hosted experiment pins that revision and its configuration. A different
revision gets a fresh experiment ID. Selection and intake require dashboard
or operator authority. Follow [Deploy to a shared host](https://coli-saar.github.io/parlando/manual/deploy/)
for exact credential setup and operator commands.

For a local WASM policy, build a separate agent ZIP with
`parlando build --manifest agent.yaml` and check that ZIP. Upload the game
ZIP first, then upload the agent ZIP to the same host using the target game's
credential:

```sh
parlando deploy --host "$HOST" --package "$GAME_PACKAGE" --json
parlando deploy --host "$HOST" --package "$AGENT_PACKAGE" --json
```

Record both returned revisions. Configure the policy factory ID in the
experiment and pass the game revision to `parlando select`. Selection pins
the applicable agent revisions along with the game. If you add a later
game revision, deploy the agent ZIP again so the host checks compatibility
for that revision before a new experiment selects it. Open testing intake
after verifying the selected packages and intended pairing.
For an external service, use the built-in `remote_grpc` factory and
configure its endpoint in the experiment.
