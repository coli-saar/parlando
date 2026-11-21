---
title: Package a game for deployment
---

# Package a game for deployment

A deployable package is the output of `parlando build`. It contains the game rules the host executes and the files participants load in their browsers. Copy the whole `dist/parlando/` directory when moving a build to another machine.

```text
dist/parlando/
  game.yaml
  game.wasm
  client/
    index.html
    assets/
```

The built manifest declares a game ID, display name, version, and paths relative to the package directory.

```yaml
id: beacon
name: Beacon Relay
version: 0.1.0
component: game.wasm
client_dist: client
```

The project-level `game.yaml` points to build inputs and outputs. The manifest inside `dist/parlando/` describes the finished artifact. Pass the built manifest when you want to validate the exact package you will send.

```sh
parlando test
parlando build
parlando check --manifest dist/parlando/game.yaml --json
```

## Package identity

The game ID groups releases of the same game. Set a human-readable version when you change the rules or participant experience. The host computes a **package revision** from every package path and its bytes. A JavaScript change yields a new package revision even when the mechanics component stays identical. Use the revision returned at upload when selecting a package for an experiment.

An experiment on a shared host retains its package revision and configuration for the life of its ID. A changed package or configuration gets a fresh experiment ID. Earlier results retain their original rules and browser build.

## Use a custom build

Supply a WebAssembly component exporting the `game` world of `parlando:game@4.0.0`, with no host imports. A raw core WASM module needs to be wrapped as a component before Parlando can load it. The component provides the schema, initialization, action and recovery operations defined by that world. Rust and Python starter builds already provide these exports.

Supply browser files with a production `index.html` and relative asset URLs. A headless-only package can omit `client_dist`. Keep participant assets under their declared directory and reserve `api`, `ws`, `admin`, and `health` for the host. Include only generated runtime files. Deployment accepts the manifest, the mechanics component, and declared browser assets.

[Deploy the built package](../deploy/) after the exact artifact passes the loader check.
