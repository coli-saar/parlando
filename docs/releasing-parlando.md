# Releasing Parlando

For a prepared release, run these commands from the repository root:

1. `make test-release-candidate` builds packages and tests them in temporary consumer projects.
2. `make publish` builds the six packages from the current checkout and uploads them using your registry credentials.

Both commands reuse the installed Python packaging tools.

| Registry | Packages |
| --- | --- |
| crates.io | `parlando-game-derive`, `parlando-game-sdk`, `parlando` |
| npm | `@coli-saar/parlando-client` |
| PyPI | `parlando-python-sdk`, `parlando-agent-sdk` |

All six packages use one release version. Publish every package at that
version, including SDKs whose source code has no changes since the previous
release. On crates.io, publish `parlando-game-derive`, `parlando-game-sdk`,
and `parlando` in that order.
On PyPI, `parlando-python-sdk` supplies the game API imported as
`parlando_game_sdk`, and `parlando-agent-sdk` supplies the remote agent API.

This document is the canonical release procedure. `docs/publishing-packages.md` identifies the packages and their
installation paths, while this page defines the coordinated release.

## One-time Python setup

Both release commands require Python 3.11 or newer with `build` and `twine`
installed. To check an existing installation, run the following from the
repository root. Make prints the interpreter it uses. If both checks pass,
the Python setup is complete.

```bash
make check-release-versions check-release-tools
```

By default, Make uses `.local/release-venv/bin/python` if it exists, then
falls back to `python3` on your `PATH`. You can use an existing release
environment without activating it.

If you see `ModuleNotFoundError: No module named 'tomllib'`, check the
interpreter printed by Make. Python 3.11 introduced `tomllib` in the
standard library. If that interpreter is from your active Conda `base`
environment, upgrade it once:

```bash
conda install -n base "python=3.11"
```

If the packaging check reports a missing `build` or `twine` module, use
the interpreter printed by Make to install them. For example, if Make
prints `python3`, run `python3 -m pip install build twine`.

## 1. Establish the release version

Run `make check-release-versions` to verify that the six manifests, the Rust
SDK dependency pins and the npm lockfile agree. Check the latest published
versions on crates.io, npm, and PyPI. Use the existing server and client
versions to establish the baseline for a package's first release.

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

Pin dependencies between the three Parlando crates to the exact shared version. Update first-party
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

Install Hugo and the `wasm32-unknown-unknown` Rust target, and put
`wasm-tools` on `PATH`. Run the candidate target from the repository root:

```bash
make test-release-candidate
```

The target runs `make test`, `make test-e2e`, `make test-components`, and
`make test-runtime-communication`. Read the browser and Prolific results in
`target/e2e-report.md`. The communication gate runs four concurrent fixture
games for each combination of typed or speech communication and human–human
or human–agent pairing. All four runs must pass.
Each writes a JSON report under `target/runtime-stress/`. Longer capacity
runs are described in [runtime stress testing](runtime-stress-testing.md).

The candidate rehearsal snapshots the package inputs, builds all six packages,
and installs them in temporary consumer projects. Cargo installs the CLI from its extracted `.crate` archive
and resolves the Rust SDK and derive dependencies from their candidate archives.
The starter clients install the npm tarball. A fresh virtual environment
installs both Python SDK wheels and runs their tests. The installed CLI creates,
tests, and builds the Rust and Python Beacon Relay starters, then deploys
each on a loopback host. Chromium signs in on the administrator
origin, opens the game dashboard, and starts a local preview. For each starter,
two independent Chromium sessions exchange messages in both
directions, check that each message appears once in each browser, and verify
that the client never requests the microphone. Both participants then play
the game to completion. Reloading one browser verifies that the result persists.
The dashboard stops the preview after the session ends.

The rehearsal also builds a standalone Beacon agent against the packaged
`parlando-game-sdk`. The installed CLI builds and checks its independent
agent ZIP, then runs it in both seats of a headless Rust Beacon game with
default fuel metering and again with metering disabled. Both runs use the same
three seeds and request no timed wakeups. Each must reach the expected completion and
record the packaged policy's WASM digest for both seats. On the loopback
host, the CLI installs the game first and uploads the agent separately.
The dashboard selects the agent factory for a human–agent experiment, and
Chromium plays with that policy.

A second Beacon policy uses the generated game bindings. It runs alongside
the first agent with a pinned game ZIP while the source is unavailable.
Selecting another ZIP changes the completion target. A third ZIP changes
the action schema. The CLI regenerates the bindings, and the policy's
obsolete action fails to compile. Selecting the original ZIP restores its
bindings and completion target.

The rehearsal installs `componentize-py` and Playwright as test dependencies.
Set `PLAYWRIGHT_BROWSERS_PATH` to reuse a compatible Chromium installation.
The rehearsal may fetch public dependencies and runs without registry
credentials. The npm tarball and Python distributions are built under `target/release-packages/`. Both Make targets rebuild them on each invocation. The candidate
target also builds the optimized server and JavaScript client, validates the Hugo
manual, and runs the registry dry runs.

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
make publish
```

Run publication from the committed source revision you tested. The two Make
targets are independent and can run in separate checkouts of that revision.
Publication checks the coordinated versions and release tools, then Cargo packages
and uploads the derive crate, SDK and server in dependency order. npm builds
the client from its lockfile and uploads the new tarball. Each Python SDK gets
a fresh source distribution and wheel, which Twine checks before upload.
An upload failure stops subsequent packages.

If npm requests browser approval, keep the process alive while the
maintainer follows its link. Verify each exact published version in its
registry. After a partial release, retry only the failed and pending
targets. Use `publish-rust-derive`, `publish-rust-sdk`,
`publish-rust-server`, `publish-js-client`,
`publish-python-game-sdk`, or `publish-python-agent-sdk` as appropriate.
If PyPI accepted only one distribution file, verify the accepted filename and version on PyPI. Use the release Python to run `-m build --outdir DIRECTORY SDK_SOURCE`
with a fresh output directory. Run `-m twine check --strict` on both files, then
`-m twine upload --repository pypi` with only the missing file’s path.

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
