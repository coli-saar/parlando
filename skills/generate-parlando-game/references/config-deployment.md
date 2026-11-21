# Package and deploy a game

The project `game.yaml` declares game identity and build inputs.
`parlando build` stages a deployable directory at `dist/parlando/`.
Its manifest points to the exact component and optional built participant
assets. Run `parlando check --manifest dist/parlando/game.yaml` before
handing off or uploading that directory.

For a local pilot, run `parlando serve --manifest dist/parlando/game.yaml`
and open `/admin` on the local listener. Study settings such as pairing,
consent, intake, communication, and agent selection belong to the experiment.
Game-owned parameters go in the game's typed configuration. Store secrets
outside packages, browser assets, and experiment configuration.

For an authorized deployment, use the shared dashboard or
`parlando deploy --server <origin> --package dist/parlando --json`.
The CLI uses a game-scoped upload credential. Record the returned revision.
A hosted experiment pins that revision and its configuration. A different
revision gets a fresh experiment ID. Selection and intake require dashboard
or operator authority. Follow [Deploy to a shared host](https://coli-saar.github.io/parlando/manual/deploy/)
for exact credential setup and operator commands.
