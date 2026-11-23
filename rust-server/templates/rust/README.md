# Beacon Relay

Two players take turns adding one or two ticks of charge. Reach the target exactly
to activate the beacon.

Install Node.js with npm, the Rust target, and the WebAssembly packaging tool.

```sh
rustup target add wasm32-unknown-unknown
cargo install --locked wasm-tools
parlando test
parlando build
parlando check --package dist/__GAME_ID__-0.1.0.zip
```

Follow the [local pilot walkthrough](https://coli-saar.github.io/parlando/manual/start/getting-started/#start-a-local-server)
to start a server, deploy `dist/__GAME_ID__-0.1.0.zip`, select its revision for an
experiment, and open testing intake. Use the game ID from your `game.yaml` and enter from
two separate browser profiles to play both seats.

Edit the rules and role views in `src/lib.rs`, the participant application in
`client/main.jsx`, and the scenario in `scenarios.yaml`. Build and upload each changed package before selecting its revision.

See the [experimenter manual](https://coli-saar.github.io/parlando/manual/) for
package contents, shared-host deployment, and experiment operation.
