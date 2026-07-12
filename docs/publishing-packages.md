# Publishing Packages

This page is for publishing the reusable Parlando packages:

- `parlando-server`: Rust library crate under `rust-server`, published to crates.io.
- `@parlando/client`: browser SDK under `js-client`, published locally with yalc.

The demo `parlando-space-game` crate and `space-game` browser app are examples, not packages we publish as part of the reusable platform release.

## Before Publishing

Use one version for both reusable packages unless there is a deliberate reason not to. Today the version is set in:

- `rust-server/Cargo.toml` under `package.version`
- `js-client/package.json`

Before an online publish:

```bash
git status --short
make publish-dry-run
```

Publish the Rust crate from a clean tree after tests and package dry runs pass. crates.io does not let you overwrite a published version, so bump the Rust version before retrying a release that already reached the registry.

## Local Package Smoke Test

Use this when checking the package shape before involving crates.io:

```bash
make publish-local
```

This runs:

```bash
cd rust-server && cargo package --allow-dirty
cd js-client && npm run yalc
```

`cargo package` validates the Rust crate contents and writes a local `.crate` archive under `rust-server/target/package/`. `npm run yalc` builds `@parlando/client` and publishes it to the local yalc store so sibling game clients can install the package-shaped SDK without using GitHub Packages.

For only one package:

```bash
make package-rust-server-local
make publish-js-client-local
```

## Rust Online Dry Run

Use this immediately before the real publish:

```bash
make publish-dry-run
```

This runs:

```bash
cd rust-server && cargo publish --dry-run --allow-dirty
```

The dry run catches missing manifest metadata, files excluded from the package, and registry/authentication assumptions before pushing a real Rust version. It allows a dirty tree so you can run it while preparing a release branch; the real publish command keeps Cargo's normal clean-tree guard.

## Publish Rust Online

Rust goes to crates.io:

```bash
cargo login
make publish-rust-server
```

There is intentionally no online publish command for `@parlando/client`. The JS side is local-only through `make publish-js-client-local` / `npm run yalc`.

## Consumer Versions

Rust game crates should depend on the released crate once it is published:

```toml
parlando-server = "0.1.0"
```

During local development, use a path dependency:

```toml
parlando-server = { path = "../../rust-server" }
```

Browser clients can keep a versioned dependency for package-shaped local development:

```json
"@parlando/client": "^0.1.0"
```

Use yalc only for local unpublished testing:

```bash
cd js-client && npm run yalc
cd ../space-game && yalc add @parlando/client && npm install
```
