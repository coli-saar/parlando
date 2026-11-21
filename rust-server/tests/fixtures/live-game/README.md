# Live server test fixture

This component supplies tiny finish and counter games for the live server unit
tests. The checked-in component keeps `cargo test` independent of a Wasm target
or component tooling. Regenerate it from this directory with the following
commands after changing the guest or its component SDK.

```sh
cargo build --locked --release --target wasm32-unknown-unknown
wasm-tools component new target/wasm32-unknown-unknown/release/parlando_live_game_fixture.wasm -o fixture.wasm
```
