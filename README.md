# Parlando

Parlando is a platform for running two-player online dialogue-game experiments. A game supplies a
browser interface and typed task rules; Parlando supplies participant entry, consent, pairing,
communication, agents, monitoring, structured data, privacy controls, export, and optional Prolific
recruitment.

The [Parlando manual](docs/web/content/manual/_index.md) is the self-contained starting point for installing Parlando,
running the Great Tree example, creating a game, configuring a study, deploying it, and managing
research data.

## Run the Great Tree example

Install stable Rust, Node.js 20.19 or newer (or 22.12 or newer), npm, and GNU Make. Then run:

```sh
games/great-tree/run.sh
```

Open <http://127.0.0.1:8080/admin>, create the first administrator, create an experiment, and open
its participant link in two isolated browser contexts. See
[Install and run the example](docs/web/content/manual/start/getting-started.md) for the complete walkthrough.

## Use the released packages

Games use the coordinated Rust and JavaScript releases:

```toml
[dependencies]
parlando = "0.4.1"
```

```json
{
  "dependencies": {
    "@coli-saar/parlando-client": "^0.4.1"
  }
}
```

The repository's `generate-parlando-game` skill can generate a complete Rust server, React
participant application, tests, and deployment files from a task description. The
[Create a game](docs/web/content/manual/build/create-game.md) chapter explains the workflow.

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

Parlando 0.4.1 supports roles `A` and `B`, live human–human and human–agent sessions, headless
agent–agent evaluation, SQLite persistence, typed communication, optional live voice and
transcription, and single-process live session state. Different compiled games run as separate
processes with separate databases.

Parlando is distributed under the terms in [LICENSE](LICENSE).
