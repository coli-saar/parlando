# Parlando packages

The coordinated [release procedure](releasing-parlando.md) describes how
maintainers package, verify, and publish the CLI and SDKs.

| Registry | Package | Purpose |
| --- | --- | --- |
| crates.io | `parlando` | Installs the CLI and provides the library for custom hosts. |
| crates.io | `parlando-game-sdk`, `parlando-game-derive` | Build Rust games and typed agents for native or WebAssembly execution. |
| npm | `@coli-saar/parlando-client` | Supplies the browser participant application and client bindings. |
| PyPI | `parlando-game-sdk` | Builds Python game components through the Python SDK. |
| PyPI | `parlando-remote-agent` | Runs the Python service for a remote agent. |

For version 0.7.0, the PyPI distribution names are `parlando-python-sdk` and `parlando-agent-sdk`, respectively.

Use `cargo install --locked parlando --bin parlando` for the CLI. New games
get matching SDK dependencies from `parlando new`. Commit the project
dependency manifests and lockfiles so your build can resolve the same package
versions. See [the experimenter manual](web/content/manual/_index.md) for
the workflow from the installed command to the game package.
