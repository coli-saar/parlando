---
name: release-parlando
description: Prepare, validate, and publish a coordinated Parlando release of the Rust `parlando` crate and `@coli-saar/parlando-client` npm package. Use when bumping Parlando versions, auditing release references and migration guidance, running the full release test and packaging matrix, performing registry dry runs or real publishes, verifying published artifacts, or handing off Git tag and GitHub push commands.
---

# Release Parlando

Treat the Rust crate and JavaScript client as one coordinated release. Work from the repository root and keep an explicit target version throughout the run.

## Establish the release

1. Read `AGENTS.md`, `Makefile`, `docs/publishing-packages.md`, both package manifests, `CHANGELOG.md`, and the target migration guide.
2. Parse the requested stable SemVer version. If it is absent, ask for it; never infer a publish version.
3. Identify the previous coordinated release from the registries and `CHANGELOG.md`. Query crates.io and npm independently, and stop if their stable versions disagree.
4. Check whether the target version already exists on either registry. Since published versions are immutable, stop and report any collision before editing or publishing.
5. State the active phase: `prepare`, `verify`, or `publish`. A resumed invocation must re-run the checks for its phase rather than trusting an earlier transcript.

Do not run Git commands unless the user separately authorizes the specific Git operation, as required by this repository. Do not work around a dirty checkout with `cargo publish --allow-dirty`. Registry publication is irreversible: perform it only when the user explicitly requested actual publication, not merely release preparation or a dry run.

## Prepare files

Update the release as one consistent change:

- Set `rust-server/Cargo.toml` and the root version entries in `js-client/package.json` and `js-client/package-lock.json` to the target.
- Update every first-party game manifest that consumes `parlando` or `@coli-saar/parlando-client`. Refresh affected lockfiles with the package manager; do not hand-edit resolved registry integrity metadata. If the target is not published yet and a registry lockfile cannot be refreshed, defer that lockfile until after publication and record the temporary limitation.
- Update current examples in `docs/publishing-packages.md`. Preserve old versions in historical changelog entries and older migration guides.
- Update `skills/generate-parlando-game/SKILL.md` to name the target as the current coordinated release while retaining the rule that Rust and JavaScript minor versions must match. Fix any obsolete migration-guide link in that skill.
- Review current user-facing docs for statements that became stale after the manifest update, especially `docs/cross-compiling-for-linux.md`.
- Move the release notes from `Unreleased` into `## [<version>] - YYYY-MM-DD` in `CHANGELOG.md`. Leave a new empty `Unreleased` section above it.
- Update or create `docs/migrating-<previous>-to-<target>.md` for a breaking release. Include coordinated dependency bumps, required data backup/conversion, public Rust and JavaScript API changes, configuration/protocol changes, removed names without compatibility aliases, and an end-to-end validation checklist. Calibrate claims against the current source and release notes; do not certify completeness from the filename alone.
- Record durable release-process choices in `notes/technical-decisions.md`, not in `docs/`.

Run the deterministic audit after editing:

```bash
python3 skills/release-parlando/scripts/check_release.py <version>
```

Resolve every failure. Also inspect broad searches for the previous version and old API names, classifying each match as current, historical, test fixture, or transitive dependency. Never mechanically replace historical or third-party versions.

## Verify the release candidate

Run in this order and report the exact failing command if a gate stops:

```bash
make test
cargo build --release --all-features --manifest-path rust-server/Cargo.toml
npm --prefix js-client run build
make package-local
make publish-dry-run
```

Inspect the Cargo package and npm dry-run file lists for secrets, private configuration, internal notes, missing declarations, and unexpected generated files. Confirm registry identities before publication with `cargo owner --list parlando` and `npm owner ls @coli-saar/parlando-client` or equivalent read-only identity checks.

If credentials are absent or expired, ask the user to authenticate with `cargo login` or `npm login`; never request, print, or store a registry token.

Preparation usually leaves tracked changes. Because Cargo's real publish must use a committed, clean source tree, stop after successful dry runs and give the user the exact files to review plus suggested `git status`, `git diff`, `git add`, and `git commit` commands. Do not execute them. Resume at the publish phase after the user commits the candidate.

## Publish and verify

Proceed only when actual publication was explicitly requested and both packages still pass the target-version audit and dry runs from the committed candidate. Re-run `cd rust-server && cargo publish --dry-run` without `--allow-dirty`; its clean-source check is the publication gate even when Git commands are unavailable to the agent.

1. Recheck that the target is absent from both registries.
2. Publish Rust with `make publish-rust-server`.
3. Poll crates.io until the exact target is visible; do not mistake index propagation delay for failure.
4. Publish JavaScript with `make publish-js-client`.
5. Poll npm until the exact target is visible and its `latest` tag resolves to it.
6. Run clean consumer-resolution checks for both packages in a temporary directory. Confirm the installed manifests report the target and build a minimal Rust and TypeScript consumer when practical.
7. Re-run the release audit. If the first publication succeeds and the second fails, report the partial release prominently and retry only the unpublished package; never republish or overwrite the successful one.

Do not publish the Git tag before both registries and consumer checks succeed.

## Hand off Git and GitHub steps

After both packages are verified, provide commands for the user to run, substituting the actual release branch:

```bash
git status --short
git tag -a v<version> -m "Parlando <version>"
git push origin <release-branch>
git push origin v<version>
```

Explain that the tag must point at the exact committed candidate that produced the packages. If GitHub Releases are used, suggest creating the release from `v<version>` and copying the matching changelog section. Do not run these Git commands unless the user explicitly authorizes them.

## Report

End with the target version, registry precheck, changed release files, each test/build/dry-run result, package contents reviewed, publication URLs and verification, any deferred consumer lockfile refresh, and the exact Git/tag commands still owned by the user.
