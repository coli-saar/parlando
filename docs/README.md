# Parlando Documentation

These pages explain how to build, run, deploy, and analyze dialogue-game experiments with Parlando. The root [README](../README.md) gives the quick research overview; this directory is the user-facing technical manual.

If you are starting a new experiment, read these pages in order:

1. [Architecture](architecture.md): how the reusable server, game crate, browser client, storage, audio services, and agents fit together.
2. [Building A Game](building-games.md): how to model state, actions, observations, validation, transitions, events, and completion.
3. [Browser Client Protocol](client-protocol.md): what JSON the JavaScript client sends and receives, including role-specific observations and actions.
4. [Agents](agents.md): how to add in-process Rust agents or connect Python agents over gRPC.
5. [Running And Deployment](running-and-deployment.md): local development, configuration, frontend serving, Docker, Render, and remote-agent deployment.
6. [Data And Monitoring](data-and-monitoring.md): persisted evaluation data, operator monitoring, and export.

Maintenance reference:

- [Publishing Packages](publishing-packages.md): local package smoke tests and online publishing for `parlando-server` and `@coli-saar/parlando-client`.

Use the demo game as a worked example:

- `space-game/server` shows a complete adapter, state engine, server binary, and agent selector.
- `space-game/client` shows a browser UI built on the reusable JavaScript client.

The demo is intentionally not the center of the platform. A real study should define its own state, actions, observations, participant UI, and analysis-oriented completion summary.
