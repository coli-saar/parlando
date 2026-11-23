# <img src="brand/parlando-wordmark.svg" width="260" alt="Parlando">

[![parlando release](https://img.shields.io/crates/v/parlando?label=parlando)](https://crates.io/crates/parlando)
[![parlando-game-sdk release](https://img.shields.io/crates/v/parlando-game-sdk?label=parlando-game-sdk)](https://crates.io/crates/parlando-game-sdk)
[![parlando-game-derive release](https://img.shields.io/crates/v/parlando-game-derive?label=parlando-game-derive)](https://crates.io/crates/parlando-game-derive)
[![@coli-saar/parlando-client release](https://img.shields.io/npm/v/@coli-saar/parlando-client?label=%40coli-saar%2Fparlando-client)](https://www.npmjs.com/package/@coli-saar/parlando-client)
[![parlando-python-sdk release](https://img.shields.io/pypi/v/parlando-python-sdk?label=parlando-python-sdk)](https://pypi.org/project/parlando-python-sdk)
[![parlando-agent-sdk release](https://img.shields.io/pypi/v/parlando-agent-sdk?label=parlando-agent-sdk)](https://pypi.org/project/parlando-agent-sdk)

Parlando runs two-player dialogue-game experiments with humans and software agents. A game package contains WebAssembly mechanics and a browser application. A shared host serves one current package per game and opens a separate dashboard for each game’s experiments.

```sh
cargo install --locked parlando --bin parlando
```

Start with the [experimenter manual](https://coli-saar.github.io/parlando/manual/). Its [first walkthrough](https://coli-saar.github.io/parlando/manual/start/getting-started/) takes you from installation to playing both seats of a local game. Follow [package a game](https://coli-saar.github.io/parlando/manual/build/packages/) and [deploy to a shared host](https://coli-saar.github.io/parlando/manual/deploy/) when you are ready to run a study.

The games in this checkout each provide `build.sh` and `deploy.sh`. For example, from `games/great-tree/`:

```sh
./build.sh
parlando login --server lab
./deploy.sh --host lab
```

Use your server URL or alias in place of `lab`. Login is needed once per server. The deployment uploads the ZIPs from the last build, game first, then agents. Use `./build.sh --game-only` for a game-only upload, or omit `--host` to deploy to the CLI’s default server.

To host the server and deploy game and agent packages, see [Deploy Parlando on Railway](docs/web/content/manual/deploy/railway.md).

For maintainers, see the [release procedure](docs/releasing-parlando.md) and the [internals atlas](developer/code-atlas/index.html).

## Preview the manual

```sh
hugo server --source docs/web
```

Open [the local site](http://127.0.0.1:1313/parlando/).
