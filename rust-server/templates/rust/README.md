# Beacon Relay

Two players take turns adding one or two ticks of charge. Reach the target exactly
to activate the beacon.

Install Node.js with npm, the Rust target, and the WebAssembly packaging tool.

```sh
rustup target add wasm32-unknown-unknown
cargo install --locked wasm-tools
parlando test
parlando build
parlando serve --manifest dist/parlando/game.yaml
```

Open [the local dashboard](http://127.0.0.1:8000/admin). Create an administrator
and an experiment, then start **Local preview**. Enter from two separate browser
profiles and play both seats.

Edit the rules and role views in `src/lib.rs`, the participant application in
`client/main.jsx`, and the scenario in `scenarios.yaml`. Build again before
serving or uploading a changed package.

See the [experimenter manual](https://coli-saar.github.io/parlando/manual/) for
package contents, shared-host deployment, and experiment operation.
