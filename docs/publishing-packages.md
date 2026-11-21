# Publishing Packages

This page is for maintainers publishing Parlando packages and for game authors deciding how to depend on them. Most game projects should use released registry packages; local paths are only for temporary debugging.

For a coordinated Parlando release, follow [Releasing Parlando](releasing-parlando.md). That process
defines version selection, changelog and migration requirements, complete test gates, interactive
registry authentication, publication, and post-publication verification.

Parlando publishes reusable packages across three registries:

- `parlando`: Rust library crate under `rust-server`, published to crates.io.
- `parlando-game-derive` and `parlando-game-sdk`: the Rust Game SDK and its derive macro, published to crates.io.
- `@coli-saar/parlando-client`: browser SDK under `js-client`, published to npm.
- `parlando-python-sdk` and `parlando-agent-sdk`: the Python game and remote agent SDKs, published to PyPI.

All six packages carry the same version in each coordinated release.

The intended dependency model is deliberately simple:

- Normal game repositories depend on released packages from the online registries.
- While debugging a Parlando bug locally, a game may temporarily depend on absolute local paths.
- Local path dependencies are development edits. Do not commit them to a game release branch.

## Package Registries

### Rust: crates.io

Rust libraries are distributed as crates. `parlando` is published to crates.io, the default Cargo registry. A game normally writes:

```toml
parlando = "0.6.0"
```

Cargo resolves that version from crates.io and records the exact selected version in `Cargo.lock`. `cargo publish --dry-run` checks the crate contents, builds the packaged crate, and verifies what would be uploaded. `cargo publish` uploads the crate. crates.io versions are immutable, so a version cannot be overwritten after publishing.

### JavaScript: npm

Browser SDK packages are distributed through npm. `@coli-saar/parlando-client` is published to the public npm registry under the `coli-saar` scope. A game normally writes:

```json
"@coli-saar/parlando-client": "^0.6.0"
```

npm resolves that dependency from the configured npm registry, normally `https://registry.npmjs.org/`, and records the exact package tarball and integrity in `package-lock.json`. `npm publish --dry-run` runs the package `prepack` script, builds the SDK, creates the tarball, and reports what would be uploaded. `npm publish` uploads the package. Published npm versions are effectively immutable for normal release workflow, so bump the package version before each release.

`js-client/package.json` sets:

```json
"publishConfig": {
  "access": "public"
}
```

That makes the scoped npm package public when it is published.

## Normal Game Dependencies

Use released registry packages in normal game repositories:

```toml
parlando = "0.6.0"
```

```json
"@coli-saar/parlando-client": "^0.6.0"
```

This is the only dependency style that should be committed for a game release or deployment. It gives clean provenance through the package manifests and lockfiles:

```bash
cargo tree -i parlando
npm ls @coli-saar/parlando-client
```

## Local Parlando Bugfix Testing

If a game exposes a bug in Parlando, fix Parlando in a local checkout and temporarily point the game at that checkout.

Prefer absolute paths so the provenance is obvious and does not depend on where another developer checked out the repositories.

Rust game server:

```toml
# Temporary local debug dependency. Do not commit to release branches.
parlando = { path = "/absolute/path/to/parlando/rust-server" }
```

JavaScript game client:

```json
"@coli-saar/parlando-client": "file:/absolute/path/to/parlando/js-client"
```

Because the JavaScript SDK exports files from `dist`, build the local SDK before installing it into the game:

```bash
cd /absolute/path/to/parlando/js-client
npm install
npm run build

cd /absolute/path/to/game/client
npm install
npm run build
```

When the bugfix is ready for release, publish Parlando normally, switch the game back to registry dependencies, refresh lockfiles, and verify the game still builds.

## Local Package Shape Checks

Use package-shape checks when preparing a Parlando release or when a local path worked but you want to verify that the packaged artifact also contains the right files.

```bash
make package-local
```

This runs:

```bash
cd rust-server && cargo package --allow-dirty
cd js-client && npm pack --dry-run
```

`cargo package` validates the Rust crate archive under `rust-server/target/package/`. `npm pack --dry-run` builds the JavaScript SDK through `prepack` and reports the npm tarball contents without publishing.

For only one package:

```bash
make package-rust-server-local
make package-js-client-local
```

## Online Dry Run

Set up the Python release environment, then run:

```bash
make publish-dry-run PYTHON=.local/release-venv/bin/python
```

The dry run checks Cargo and npm packaging and builds and checks both
Python distributions. [Release tools and prerequisites](releasing-parlando.md#3-run-the-release-gates)

## Publish Online

Use `make publish` for the coordinated release with the configured registry
credentials. Individual package targets are available for recovering a
partial release. [Release procedure](releasing-parlando.md#5-publish-the-release)

## Switching Back From Local Paths

Before committing a game release, remove temporary local dependencies:

```toml
parlando = "0.6.0"
```

```json
"@coli-saar/parlando-client": "^0.6.0"
```

Then refresh and verify:

```bash
cargo update -p parlando
npm install
cargo tree -i parlando
npm ls @coli-saar/parlando-client
```

The manifest and lockfile should no longer contain `/absolute/path/to/parlando`, `file:`, or local path references.
