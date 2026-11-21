# Releasing Parlando

`make publish` covers six Parlando packages across crates.io, npm, and PyPI.

| Registry | Packages |
| --- | --- |
| crates.io | `parlando-game-derive`, `parlando-game-sdk`, `parlando` |
| npm | `@coli-saar/parlando-client` |
| PyPI | `parlando-python-sdk`, `parlando-agent-sdk` |

All six packages use one release version. Publish every package at that
version, including SDKs whose source code has no changes since the previous
release. On crates.io, `parlando-game-derive` must be available before
`parlando-game-sdk`, which in turn must be available before `parlando`.
On PyPI, `parlando-python-sdk` supplies the game API imported as
`parlando_game_sdk`, and `parlando-agent-sdk` supplies the remote agent API.

This document is the canonical release procedure. `docs/publishing-packages.md` identifies the packages and their
installation paths, while this page defines the coordinated release.

## 1. Establish the release version

Run `make check-release-versions` to verify that the six manifests, the Rust
SDK dependency pins and the npm lockfile agree. Check the latest published
versions on crates.io, npm, and PyPI. When moving from an earlier
server-and-client release to the first six-package release, use the existing
server and client versions to establish the baseline.

Unless the maintainer selects another version, add one to the patch component:

```text
0.7.0 → 0.7.1
```

Confirm that the target version is available for each package on its
registry before preparing the release. If some packages already carry that
version as part of the same release, follow the partial release procedure.

## 2. Prepare one coordinated candidate

Set the shared version in the six package manifests:

- `rust-server/Cargo.toml`, `parlando-game-sdk/Cargo.toml`, and
  `parlando-game-sdk/derive/Cargo.toml`;
- `js-client/package.json` and the root entries in `js-client/package-lock.json`;
- `parlando-python-sdk/pyproject.toml` and `parlando-agent-sdk/pyproject.toml`.

Keep the server's dependency on `parlando-game-sdk` and the SDK's dependency
on `parlando-game-derive` pinned to the exact shared version. Update first-party
consumer manifests that declare released Parlando packages. Refresh lockfiles
with the package manager. If an npm consumer lockfile requires the new
version to exist on npm, record that as a deferred post-publication step.

Update the following release-facing material in the same candidate:

- move the entries under `Unreleased` in the repository-root `CHANGELOG.md` into a dated
  `## [<version>] - YYYY-MM-DD` section, then leave a new empty `Unreleased` section above it;
- review `skills/generate-parlando-game/SKILL.md` and its references for API or configuration changes;
- update current version examples and behavioral descriptions in the README and `docs/`, including
  the Hugo manual under `docs/web/content`;
- rebuild or validate generated documentation where the repository's documentation workflow
  requires it.

`CHANGELOG.md` is the centralized history for all six packages. Do not maintain separate Rust and
JavaScript release notes, and do not rewrite historical changelog sections or historical migration
guides merely because the current version changed.

### Migration guides

Create `docs/migrating-<previous>-to-<version>.md` when downstream game developers must change code,
configuration, deployment files, or databases. The guide must state:

- the coordinated Rust and JavaScript dependency updates;
- every required Rust or TypeScript API change;
- configuration and protocol changes;
- removed names or behavior, without suggesting nonexistent compatibility aliases;
- database backup and conversion steps, including validation after conversion;
- an end-to-end checklist that proves the migrated game works.

Do not create a migration guide for a release that requires no downstream action. In that case, the
changelog is sufficient and should describe the behavioral changes.

Run the deterministic release metadata audit after preparing the files:

```bash
python3 skills/release-parlando/scripts/check_release.py <version>
```

When a migration guide is required, add:

```bash
python3 skills/release-parlando/scripts/check_release.py <version> --require-migration-guide
```

Review remaining references to the previous version and classify each as current documentation,
historical documentation, a test fixture, or a transitive dependency. Do not replace versions
mechanically.

## 3. Run the release gates

Use Python 3.11 or later for the release tools. Install the
`wasm32-unknown-unknown` Rust target and put `wasm-tools` on `PATH`.
Then run the release candidate target.

```bash
python3 -m venv .local/release-venv
.local/release-venv/bin/python -m pip install build twine
make test-release-candidate PYTHON=.local/release-venv/bin/python
```

The target runs `make test`, `make test-e2e`, and `make test-components`,
including the browser and Prolific scenarios, Wasm compatibility, and the
runtime stress test suite. Read the report at `target/e2e-report.md` and require
every layer and scenario to pass. The standalone sustained-capacity programs
in [runtime stress testing](runtime-stress-testing.md) measure performance under
load and run separately.

The candidate rehearsal snapshots the package inputs, builds all six packages,
and installs them in temporary consumer projects. Cargo installs the CLI from its extracted `.crate` archive
and resolves the Rust SDK and derive dependencies from their candidate archives.
The starter clients install the npm tarball. A fresh virtual environment
installs both Python SDK wheels and runs their tests. The installed CLI creates,
tests, and builds the Rust and Python Beacon Relay starters, deploys each to a
loopback host, and opens intake. Two independent Chromium sessions play each
game to completion and verify that the outcome survives a reload.

The rehearsal installs `componentize-py` and Playwright as test dependencies.
Set `PLAYWRIGHT_BROWSERS_PATH` to reuse a compatible Chromium installation.
The rehearsal may fetch public dependencies and runs without registry
credentials. After every test passes, the target saves the candidate artifacts
and their checksums under `target/release-candidates/`. The output gives the
exact directory to inspect.

Then verify release builds, documentation, and registry dry runs.

```bash
cargo build --release --all-features --manifest-path rust-server/Cargo.toml
npm --prefix js-client run build
hugo --source docs/web --panicOnWarning --printPathWarnings
make publish-dry-run PYTHON=.local/release-venv/bin/python
```

The dry run checks both Python source distributions and wheels. When a
new Rust dependency is still absent from crates.io, it lists the dependent
crate's package contents. Run that crate's clean-source Cargo dry run after
the dependency becomes available.

Inspect the Cargo, npm and Python package contents. They must not contain credentials, private
configuration, internal notes, or unexpected generated files, and they must contain every public
declaration required by consumers.

Review the prepared changes before publication. Cargo's real publication must come from a committed,
clean candidate. An agent must not execute Git commands without the explicit authorization required
by `AGENTS.md`; when that authorization is absent, give the maintainer the exact review and commit
commands instead.

## 4. Authenticate without losing the interactive login

Registry authentication belongs to the maintainer. Ask the maintainer to run:

```bash
cargo login
cd js-client
npm login
cd ..
```

Do not request, display, or store registry tokens. Run `npm login` in an interactive terminal. npm
normally prints a URL and waits while the account owner signs in and approves the request on the npm
website. Surface that URL explicitly to the maintainer and keep the terminal process alive while
they complete the browser step.

Be patient at this point. The maintainer may not see a background notification or notice that npm
is waiting for a browser confirmation. Wait for their confirmation or for the existing command to
finish; do not interpret a quiet terminal as failure and do not repeatedly restart `npm login`. If
the URL expires, say so and start one new login to obtain a fresh URL.

After login, verify the active npm identity and package ownership:

```bash
npm whoami
npm owner ls @coli-saar/parlando-client
cargo owner --list parlando
```

Stop before publication if the authenticated accounts are not package owners.

## 5. Publish the release

Actual publication requires an explicit request from the maintainer.
Recheck target versions and registry access immediately before uploading.
Configure Twine for PyPI using the maintainer's credentials.

```bash
make publish PYTHON=.local/release-venv/bin/python
```

The command verifies the saved candidate's version, artifact checksums, and
package inputs before uploading. It publishes the Rust derive crate, Rust
Game SDK, and server in dependency order, then uploads the exact tested npm
tarball and Python distributions. Cargo creates fresh crates.io archives from
the verified inputs and validates their registry dependency resolution. A
failed upload stops the sequence. Run the candidate target again if the package
inputs or artifacts change before publication.

If npm requests browser approval, keep the process alive while the
maintainer follows its link. Verify each exact published version in its
registry. After a partial release, retry only the failed and pending
targets. Use `publish-rust-derive`, `publish-rust-sdk`,
`publish-rust-server`, `publish-js-client`,
`publish-python-game-sdk`, or `publish-python-agent-sdk` as appropriate.
If PyPI accepted only one distribution file, verify it and upload only the
missing file from the saved candidate directory with Twine.

## 6. Verify consumers and finish the release

Create clean temporary Rust and TypeScript consumers that resolve the published version from their
registries. Confirm that their installed manifests report the target version and that minimal
consumers build. Refresh deferred first-party npm lockfiles and require registry tarball URLs and
registry-provided integrity hashes rather than local `file:` links.

Run the final audit:

```bash
python3 skills/release-parlando/scripts/check_release.py <version> --published
```

If the release requires a migration guide, include `--require-migration-guide` as well.

For each Python SDK version included in the release, install it from PyPI
in a clean environment and verify its metadata and import
(`parlando_game_sdk` or `parlando_agent_sdk`).

Tag only after the registry and clean-consumer checks succeed. When registry-derived lockfiles
could not be created before publication, use a release-candidate commit for the published artifacts
and a release-completion commit for those lockfiles; the six publishable package
trees must be identical in both commits. Tag the completion commit.

Ask the maintainer to create and push the annotated release tag. Substitute the actual released
version in both commands; for example, release `0.4.3` uses tag `v0.4.3`:

```bash
git tag -a v<version> -m "Parlando <version>"
git push origin v<version>
```

Do not merely say “push the tag” and do not suggest `git push --tags`. Give the maintainer the
specific single-tag push command with the real version, such as:

```bash
git push origin v0.4.3
```

An agent must not execute either Git command unless the user explicitly authorizes that exact Git
operation. Without that authorization, ask the user to run the displayed commands and wait for
confirmation before treating the repository release as complete.

The final release report should record:

- the previous and released versions;
- the changelog and migration-guide status;
- every test, build, documentation, package, and dry-run result;
- the reviewed package contents and any warnings;
- published package URLs and clean-consumer results;
- deferred lockfile resolution;
- the artifact-provenance and tag commits;
- any Git, tag, push, or GitHub Release commands still owned by the maintainer.
