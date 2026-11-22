---
title: Package a game for deployment
---

# Package a game for deployment

For the Beacon starter, `parlando build` creates **`dist/beacon-0.1.0.zip`**, a single file containing the game metadata, the WebAssembly rules, and any browser assets. The filename combines the game ID and version from the project’s `game.yaml`. After the compiler and browser logs, the final line gives the absolute path to the ZIP. Pass that file to `parlando deploy --package` or select it in the dashboard to upload the build.

```text
dist/beacon-0.1.0.zip
  game.yaml
  game.wasm
  client/
    index.html
    assets/
```

The built manifest declares a game ID, display name, version, and paths relative to the archive root.

```yaml
id: beacon
name: Beacon Relay
version: 0.1.0
component: game.wasm
client_dist: client
```

Edit `name` in the project’s `game.yaml` to change the display name. **Games** lists **Beacon Relay** with the game ID `beacon`. Click the ID to open `/beacon` on the administrator origin. Games can share a name because their URLs and research databases are keyed by ID. Build and deploy the updated ZIP for the new name to appear.

The project-level `game.yaml` points to build inputs and outputs. The manifest at the root of `dist/beacon-0.1.0.zip` describes the finished artifact. Pass the ZIP when you want to validate the exact package you will send.

```sh
parlando test
parlando build
parlando check --package dist/beacon-0.1.0.zip --json
```

## Build an agent ZIP

An agent has its own manifest and archive. The manifest names the game it
targets and the factory ID exported by the policy. Use `cargo_manifest` to build
from Rust source, with `features` for any policy-specific Cargo features.

```yaml
game: counterweight-hotel
id: counterweight_hotel.porter
name: Hotel porter
version: 11.0.0
cargo_manifest: server/Cargo.toml
features: [agent-component]
```

```sh
parlando build --manifest agent.yaml
parlando check --package dist/agent-counterweight_hotel.porter-11.0.0.zip
```

For a prebuilt policy, use `component: policies/porter.wasm` in place of the
Cargo fields. The agent ZIP contains `agent.yaml` and `agent.wasm`. Deploy
the game ZIP first, then [upload the agent ZIP](../deploy/#upload-a-local-agent)
and choose its factory in the experiment. See [write a typed Rust agent](https://github.com/coli-saar/parlando/blob/main/docs/agents.md#write-a-typed-rust-agent)
for a complete Beacon policy or [attach a remote agent](agents-and-voice/)
for a separate service.

## Package identity

The game ID groups releases of the same game. Set a human-readable version when you change the rules or participant experience. The host computes a **package revision** from every package path and its bytes. A JavaScript change yields a new package revision even when the mechanics component stays identical. Record the revision returned by the upload to identify the exact ZIP used in a study.

A shared host serves the current package for each game ID. Replacing the package keeps the game's study records. Keep the original ZIP if you need to reproduce sessions recorded with that build.

## Use a custom build

Supply a WebAssembly component exporting the `game` world of `parlando:game@4.0.0`, with no host imports. A raw core WASM module needs to be wrapped as a component before Parlando can load it. The component provides the schema, initialization, action and recovery operations defined by that world. Rust and Python starter builds already provide these exports.

Supply browser files with a production `index.html` and relative asset URLs. A headless-only package can omit `client_dist`. Keep participant assets under their declared directory and reserve `api`, `ws`, `admin`, and `health` for the host. Include only generated runtime files. A game ZIP contains its manifest, game component, and declared browser assets.

[Deploy the built package](../deploy/) after the exact artifact passes the loader check.
