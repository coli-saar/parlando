# Beacon Relay

Two players alternate adding one or two ticks of charge. The beacon activates
when the team reaches the target exactly.

Run these commands from this directory with the `wasm32-unknown-unknown` Rust
target, `wasm-tools`, and Node.js with npm installed.

```sh
parlando check
parlando test
parlando build
parlando serve
```

Open the administration dashboard at http://127.0.0.1:8000/admin. Create an
experiment for this game, open it for intake, and copy its participant URL.
Open the URL in two separate browser profiles, enter the room on each, and take
turns adding charge.

Edit the game rules and role views in `src/lib.rs`, the participant interface
in `client/main.jsx`, and the expected game behavior in `scenarios.yaml`.
Build again before serving changes.
