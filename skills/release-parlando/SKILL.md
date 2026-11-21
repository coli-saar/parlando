---
name: release-parlando
description: Prepare, validate, and publish a coordinated Parlando release of all six Parlando registry packages. Use when bumping Parlando versions, auditing release references and migration guidance, running correctness and E2E gates, packaging, performing registry dry runs or real publishes, recovering a partial release, or handing off Git tag and GitHub push commands.
---

# Release Parlando

Use one version for all six registry packages, including SDKs whose source
code has no changes. Work from the repository root and keep the target version explicit.

Read and follow `docs/releasing-parlando.md`; it is the canonical release procedure. This skill
adds operational guardrails but must not contradict that document.

## Establish the release

1. Read `AGENTS.md`, `docs/releasing-parlando.md`, `Makefile`, `docs/publishing-packages.md`, all six package manifests, `CHANGELOG.md`, and any target migration guide.
2. Parse a requested stable SemVer version. If the user did not specify one, increment the current coordinated version by `0.0.1` and state the inferred target before editing.
3. Identify the previous coordinated release from the registries and `CHANGELOG.md`. Check crates.io, npm, and PyPI using the published baseline described in the runbook.
4. Check whether the target version already exists on its registry. Since published versions are immutable, stop and report any collision before editing or publishing.
5. State the active phase: `prepare`, `verify`, or `publish`. A resumed invocation must re-run the checks for its phase rather than trusting an earlier transcript.

Do not run Git commands unless the user separately authorizes the specific Git operation, as required by this repository. Do not work around a dirty checkout with `cargo publish --allow-dirty`. Registry publication is irreversible: perform it only when the user explicitly requested actual publication, not merely release preparation or a dry run.

## Prepare files

Update the release as one consistent change:

- Set the version in all six package manifests and the npm lockfile root to the target. Update exact Rust SDK dependency pins and run `make check-release-versions`.
- Update every first-party game manifest that consumes `parlando` or `@coli-saar/parlando-client`. Refresh affected lockfiles with the package manager; do not hand-edit resolved registry integrity metadata. If the target is not published yet and a registry lockfile cannot be refreshed, defer that lockfile until after publication and record the temporary limitation.
- Update current examples in `docs/publishing-packages.md`. Preserve old versions in historical changelog entries and older migration guides.
- Review `skills/generate-parlando-game/SKILL.md` for API and configuration changes. The CLI supplies the shared version when scaffolding projects.
- Review current user-facing docs for statements that became stale after the manifest update, especially `docs/cross-compiling-for-linux.md`.
- Move the release notes from `Unreleased` into `## [<version>] - YYYY-MM-DD` in `CHANGELOG.md`. Leave a new empty `Unreleased` section above it.
- Update or create `docs/migrating-<previous>-to-<target>.md` when downstream game developers must change code, configuration, deployment files, or databases. Include coordinated dependency bumps, required data backup/conversion, public Rust and JavaScript API changes, configuration/protocol changes, removed names without compatibility aliases, and an end-to-end validation checklist. Do not create a migration guide when the release requires no downstream action.
- Record durable release-process choices in `notes/technical-decisions.md`, not in `docs/`.

Run the deterministic audit after editing:

```bash
python3 skills/release-parlando/scripts/check_release.py <version>
```

Add `--require-migration-guide` when the release has downstream migration work.

Resolve every failure. Also inspect broad searches for the previous version and old API names, classifying each match as current, historical, test fixture, or transitive dependency. Never mechanically replace historical or third-party versions.

## Verify the release candidate

Set up the Python release environment described in the canonical release
procedure. Run these gates in order and report the exact failing command:

```bash
make test
make test-e2e
cargo build --release --all-features --manifest-path rust-server/Cargo.toml
npm --prefix js-client run build
hugo --source docs/web --panicOnWarning --printPathWarnings
make publish-dry-run PYTHON=.local/release-venv/bin/python
```

Require `target/e2e-report.md` to show every browser and Prolific scenario passing. A test-harness
failure is still a release blocker: diagnose and fix the harness or product, then rerun both the
affected gate and its own test target. Skip the runtime stress programs in a normal release; run
them only when a relevant performance, resource-limit, scheduling, or audio change requires that
evidence and the user requests it.

Inspect the Cargo package and npm dry-run file lists for secrets, private configuration, internal notes, missing declarations, and unexpected generated files. Confirm registry identities before publication with `cargo owner --list parlando` and `npm owner ls @coli-saar/parlando-client` or equivalent read-only identity checks.

Treat yanked packages and other package-manager warnings as explicit review items. Resolve them
before publication or record a concrete, accepted reason they do not invalidate the artifact.

If credentials are absent or expired, ask the user to run `cargo login` and `npm login`; never request, print, or store a registry token. Run npm authentication in a TTY, surface its browser approval URL explicitly, and keep the process alive while the user follows the link. Be patient: the user may not see a background notification and a quiet terminal normally means npm is waiting, not that login failed. Do not restart a pending login. Generate one new URL only after the previous one expires. Check `npm whoami` immediately before the npm publish. npm may require the same interactive browser approval for a publish; again show the URL, preserve the process, and wait for the user.

Preparation usually leaves tracked changes. Because Cargo's real publish must use a committed, clean source tree, stop after successful dry runs and give the user the exact files to review plus suggested `git status`, `git diff`, `git add`, and `git commit` commands. Do not execute them. Resume at the publish phase after the user commits the candidate.

## Publish and verify

Proceed only when actual publication was explicitly requested and all six packages still pass the target-version audit and dry runs from the committed candidate. Run clean-source Cargo checks as dependencies become available in the index.

Follow the canonical publication order in `docs/releasing-parlando.md`.
The default command is `make publish`, which includes the Rust Game SDK
crates and both Python SDKs. Preserve the npm approval process if it
prompts. Verify exact versions and clean consumers at each applicable
registry. After a partial release, verify the successful uploads and run
only the failed and pending package targets.

Do not publish the Git tag before registry and consumer checks succeed.

## Hand off Git and GitHub steps

Use two commits when registry-derived consumer lockfiles cannot exist before publication:

1. Publish all six packages from the committed, clean release-candidate commit.
2. After registry verification, refresh only the deferred consumer lockfiles and record a release-completion commit. Do not change any publishable package tree between these commits.
3. Tag the release-completion commit. Report the candidate commit as artifact provenance and the tagged completion commit as the coordinated repository state.

After all six packages and deferred lockfiles are verified, provide commands for the user to run, substituting the actual release branch:

```bash
git status --short
git tag -a v<version> -m "Parlando <version>"
git push origin <release-branch>
git push origin v<version>
```

Substitute the actual version before presenting the commands. Explicitly ask the user to create the annotated tag and run the specific single-tag command `git push origin v<actual-version>`; do not say only “push the tag” and do not use `git push --tags`. Explain the two-commit boundary when it applies and require that the six publishable package trees are identical in the candidate and completion commits. If no post-publication files changed, the candidate itself is the completion commit. If GitHub Releases are used, suggest creating the release from `v<version>` and copying the matching changelog section. Do not run these Git commands unless the user explicitly authorizes them.

## Report

End with the target version, registry precheck, changelog and migration status, each unit, E2E, build, documentation, package, and dry-run result, reviewed package contents, warnings and their disposition, publication URLs, registry and clean-consumer verification, post-publication lockfile status, the artifact-provenance/tag boundary, and the exact Git/tag commands still owned by the user.
