# Configure and deploy a game

Use `game.yaml` for package identity, build inputs, and participant assets. Run `parlando test`, `parlando build` and `parlando check --manifest dist/parlando/game.yaml`. Use `parlando serve` for a local pilot.

Use `parlando host` to operate a persistent installation and `parlando deploy` to upload the package. Select the immutable package revision for each hosted experiment. Follow the CLI help and the checkout's `docs/component-hosting.md` for operator authentication, host storage, and environment variables.

Experiment configuration belongs in the dashboard. The game receives its own typed settings. Keep deployment tokens and provider credentials outside manifests, browser code, and game configuration. Give the researcher the dashboard address, participant URL, storage location, and any separate agent process command.
