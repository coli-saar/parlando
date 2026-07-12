# Parlando Documentation

These pages describe how to build, run, and analyze dialogue-game experiments with Parlando. The root [README](../README.md) gives the shorter project overview; this directory is the implementation documentation.

Read the pages in this order if you are starting a new experiment:

1. [Architecture](architecture.md): how the reusable server, game crate, browser client, storage, audio services, and agents fit together.
2. [Building A Game](building-games.md): how to model state, actions, observations, validation, transitions, events, and completion.
3. [Browser Client Protocol](client-protocol.md): what JSON the JavaScript client sends and receives, including role-specific observations and actions.
4. [Agents](agents.md): how to add in-process Rust agents or connect Python agents over gRPC.
5. [Running And Deployment](running-and-deployment.md): local development, configuration, frontend serving, Docker, Render, and remote-agent deployment.
6. [Data And Monitoring](data-and-monitoring.md): persisted evaluation data, operator monitoring, and export.

The demo game in `crates/parlando-space-game` is a complete example of a game adapter, game state engine, and agent selector. Treat it as a worked example, not as the center of the platform.
