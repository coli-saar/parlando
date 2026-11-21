# <img src="brand/parlando-wordmark.svg" width="260" alt="Parlando">

Parlando is a platform for running two-player online dialogue-game experiments. A game supplies a
browser interface and typed task rules; Parlando supplies participant entry, consent, pairing,
communication, agents, monitoring, structured data, privacy controls, export, and optional Prolific
recruitment.

The [Parlando manual](docs/web/content/manual/_index.md) is the self-contained starting point for installing Parlando,
running the Great Tree example, creating a game, configuring a study, deploying it, and managing
research data.

Maintaining Parlando itself? Open the [internals atlas](developer/code-atlas/index.html) from a local checkout to inspect runtime dependencies, session transitions, ownership and regression fixtures.

The [brand directory](brand/) contains the official Parlando logos and screen palette. After editing them, run `make sync-brand` to refresh the subproject copies.

## Build a component game

The `parlando` CLI creates a playable Rust game, tests its compiled WebAssembly
component, and packages its browser client. A shared Parlando host can run
experiments pinned to different packages. Start with [Create and play a Rust
game](docs/component-games.md#create-and-play-a-rust-game) or [Host and deploy
component games](docs/component-hosting.md).

## Run the Great Tree example

Install stable Rust, the `wasm32-unknown-unknown` target, `wasm-tools`, Node.js 20.19 or newer (or 22.12 or newer), npm, and GNU Make. Then run:

```sh
games/great-tree/run.sh
```

Open <http://127.0.0.1:8080/admin>, create the first administrator, create an experiment, and open
its participant link in two isolated browser contexts. See
[Install and run the example](docs/web/content/manual/start/getting-started.md) for the complete walkthrough.

## Author a game

Run `parlando new` to generate a Rust game using the Parlando Game SDK. The generated project includes its Wasm export, browser application, package manifest, and executable scenarios. The repository's `generate-parlando-game` skill can adapt this project to a task description. See [Create a game](docs/web/content/manual/build/create-game.md).

## Preview the manual locally

The manual is the `/manual/` section of the repository's Hugo website:

```sh
hugo server --source docs/web
```

Open <http://127.0.0.1:1313/parlando/>. Use
`hugo --source docs/web --panicOnWarning --printPathWarnings` to perform the same validation as
GitHub Actions.

The workflow in `.github/workflows/web-pages.yml` deploys the site after a successful push to
`main`. In the GitHub repository, select **Settings → Pages → Source: GitHub Actions** once to enable
the deployment. The website source lives under `docs/web`; future product pages can join the manual
without introducing a second site generator or publishing the older technical documents in `docs/`.

## Current scope

Parlando supports roles `A` and `B`, live human–human and human–agent sessions, headless
agent–agent evaluation, SQLite persistence, typed communication, optional live voice and
transcription, and single-process live session state. The shared host serves multiple Wasm games in one process. Each hosted experiment has its own database and pins its game package revision.

Parlando is distributed under the terms in [LICENSE](LICENSE).
