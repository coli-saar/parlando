# Releasing Parlando

Parlando publishes two packages as one coordinated release:

- the Rust crate `parlando` from `rust-server`;
- the JavaScript package `@coli-saar/parlando-client` from `js-client`.

Both packages use the same version. A normal release increments the current version by `0.0.1`; for
example, `0.4.2` becomes `0.4.3`. Use another version only when the maintainer explicitly requests
it. Do not intentionally publish only one package except when recovering from a partial release in
which the other package is already present on its registry.

This document is the canonical release procedure. `docs/publishing-packages.md` explains package
consumption and individual publishing commands, while this page defines the coordinated release.

## 1. Establish the release version

Read the versions in `rust-server/Cargo.toml`, `js-client/package.json`, and
`js-client/package-lock.json`. They must agree. Check crates.io and npm as well: the latest stable
versions of `parlando` and `@coli-saar/parlando-client` must agree with each other and with the
repository before preparing the next release.

Unless the maintainer selected another target, add one to the patch component:

```text
0.4.2 → 0.4.3
```

Before editing, confirm that the target version does not already exist on either registry. Registry
versions are immutable, so a collision requires a new version rather than an overwrite.

## 2. Prepare one coordinated candidate

Update the version in both publishable packages:

- `rust-server/Cargo.toml`;
- `js-client/package.json` and the root entries in `js-client/package-lock.json`.

Update every first-party game manifest that declares either package. Rust and JavaScript
dependencies must continue to identify the same Parlando release. Refresh lockfiles with Cargo or
npm rather than hand-editing resolved package checksums. An npm consumer lockfile may have to wait
until the new package exists on npm; record that as a deferred post-publication step.

Update the following release-facing material in the same candidate:

- move the entries under `Unreleased` in the repository-root `CHANGELOG.md` into a dated
  `## [<version>] - YYYY-MM-DD` section, then leave a new empty `Unreleased` section above it;
- change the current coordinated release in `skills/generate-parlando-game/SKILL.md` and review its
  references for API or configuration changes;
- update current version examples and behavioral descriptions in the README and `docs/`, including
  the Hugo manual under `docs/web/content`;
- rebuild or validate generated documentation where the repository's documentation workflow
  requires it.

`CHANGELOG.md` is the centralized history for both packages. Do not maintain separate Rust and
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

Run all ordinary, package, contract, browser, and Prolific tests:

```bash
make test
make test-e2e
```

`make test-e2e` installs Playwright's isolated Chromium when necessary and writes the combined
human-readable report to `target/e2e-report.md`. Read that report and require every layer and
scenario to pass. A harness failure is still a failed release gate until the harness or product is
fixed and the affected command is rerun.

The runtime stress programs are deliberately not part of the standard release process. They take
minutes and primarily measure sustained capacity rather than release correctness. Run them only
when a change specifically affects performance, resource limits, scheduling, or audio behavior and
the maintainer requests that evidence.

Then verify release builds, documentation, and package contents:

```bash
cargo build --release --all-features --manifest-path rust-server/Cargo.toml
npm --prefix js-client run build
hugo --source docs/web --panicOnWarning --printPathWarnings
make package-local
make publish-dry-run
```

Inspect the Cargo package and npm dry-run file lists. They must not contain credentials, private
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

## 5. Publish both packages

Actual publication is irreversible and requires an explicit request from the maintainer. Immediately
before publishing, rerun the clean-source Rust dry run and confirm that the target is still absent
from both registries:

```bash
cd rust-server
cargo publish --dry-run
cd ..
```

Publish Rust first:

```bash
make publish-rust-server
```

Wait until crates.io exposes the exact version. Then publish JavaScript in an interactive terminal:

```bash
make publish-js-client
```

The npm publish itself may also print an approval link and wait for confirmation on npm's website.
Show the link to the maintainer, keep the same process alive, and wait. Do not start another publish
while approval is pending. After npm reports success, poll the registry until the exact version is
visible and npm's `latest` tag points to it; the upload message can precede registry visibility.

If Rust succeeds and JavaScript fails, report a partial release prominently. Retry only the
unpublished JavaScript version after correcting authentication or approval. Never attempt to
republish the already published Rust version.

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

Tag only after both registries and the clean-consumer checks succeed. When registry-derived lockfiles
could not be created before publication, use a release-candidate commit for the published artifacts
and a release-completion commit for those lockfiles; the publishable Rust and JavaScript package
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
- both registry URLs and clean-consumer results;
- deferred lockfile resolution;
- the artifact-provenance and tag commits;
- any Git, tag, push, or GitHub Release commands still owned by the maintainer.
