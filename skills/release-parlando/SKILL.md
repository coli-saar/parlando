---
name: release-parlando
description: Prepare, validate, and publish a coordinated Parlando release of the Rust `parlando` crate and `@coli-saar/parlando-client` npm package. Use when bumping Parlando versions, auditing release references and migration guidance, running correctness and E2E gates, packaging, performing registry dry runs or real publishes, recovering a partial release, or handing off Git tag and GitHub push commands.
---

# Release Parlando

Treat the Rust crate and JavaScript client as one coordinated release. Work from the repository root and keep an explicit target version throughout the run.

Read and follow `docs/releasing-parlando.md`; it is the canonical release procedure. This skill
adds operational guardrails but must not contradict that document.

## Establish the release

1. Read `AGENTS.md`, `docs/releasing-parlando.md`, `Makefile`, `docs/publishing-packages.md`, both package manifests, `CHANGELOG.md`, and any target migration guide.
2. Parse a requested stable SemVer version. If the user did not specify one, increment the current coordinated version by `0.0.1` and state the inferred target before editing.
3. Identify the previous coordinated release from the registries and `CHANGELOG.md`. Query crates.io and npm independently, and stop if their stable versions disagree.
4. Check whether the target version already exists on either registry. Since published versions are immutable, stop and report any collision before editing or publishing.
5. State the active phase: `prepare`, `verify`, or `publish`. A resumed invocation must re-run the checks for its phase rather than trusting an earlier transcript.

Do not run Git commands unless the user separately authorizes the specific Git operation, as required by this repository. Do not work around a dirty checkout with `cargo publish --allow-dirty`. Registry publication is irreversible: perform it only when the user explicitly requested actual publication, not merely release preparation or a dry run.

## Prepare files

Update the release as one consistent change:

- Set `rust-server/Cargo.toml` and the root version entries in `js-client/package.json` and `js-client/package-lock.json` to the target.
- Update every first-party game manifest that consumes `parlando` or `@coli-saar/parlando-client`. Refresh affected lockfiles with the package manager; do not hand-edit resolved registry integrity metadata. If the target is not published yet and a registry lockfile cannot be refreshed, defer that lockfile until after publication and record the temporary limitation.
- Update current examples in `docs/publishing-packages.md`. Preserve old versions in historical changelog entries and older migration guides.
- Update `skills/generate-parlando-game/SKILL.md` to name the target as the current coordinated release while retaining the rule that Rust and JavaScript use the same exact release. Fix any obsolete migration-guide link in that skill.
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

Run in this order and report the exact failing command if a gate stops:

```bash
make test
make test-e2e
cargo build --release --all-features --manifest-path rust-server/Cargo.toml
npm --prefix js-client run build
hugo --source docs/web --panicOnWarning --printPathWarnings
make package-local
make publish-dry-run
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

Proceed only when actual publication was explicitly requested and both packages still pass the target-version audit and dry runs from the committed candidate. Re-run `cd rust-server && cargo publish --dry-run` without `--allow-dirty`; its clean-source check is the publication gate even when Git commands are unavailable to the agent.

1. Recheck that the target is absent from both registries.
2. Publish Rust with `make publish-rust-server`.
3. Poll crates.io until the exact target is visible; do not mistake index propagation delay for failure.
4. Confirm `npm whoami` names an owner of the package, then publish JavaScript with `make publish-js-client` in a TTY so browser approval can complete without restarting the upload.
5. Poll npm every 10–30 seconds for up to five minutes until the exact target is visible and its `latest` tag resolves to it. npm's successful-upload message can precede registry visibility; do not report success or failure from that message alone.
6. Run clean consumer-resolution checks for both packages in a temporary directory. Confirm the installed manifests report the target and build a minimal Rust and TypeScript consumer when practical.
7. Refresh deferred first-party npm consumer lockfiles from the registry. Reject local links and require the target version, registry tarball URL, and registry-provided integrity digest. Run `python3 skills/release-parlando/scripts/check_release.py <version> --published`, adding `--require-migration-guide` when applicable.
8. If the first publication succeeds and the second fails, report the partial release prominently, verify the successful package again, and retry only the unpublished package; never republish or overwrite the successful one.

Do not publish the Git tag before both registries and consumer checks succeed.

## Hand off Git and GitHub steps

Use two commits when registry-derived consumer lockfiles cannot exist before publication:

1. Publish both packages from the committed, clean release-candidate commit.
2. After registry verification, refresh only the deferred consumer lockfiles and record a release-completion commit. Do not change either publishable package tree between these commits.
3. Tag the release-completion commit. Report the candidate commit as artifact provenance and the tagged completion commit as the coordinated repository state.

After both packages and deferred lockfiles are verified, provide commands for the user to run, substituting the actual release branch:

```bash
git status --short
git tag -a v<version> -m "Parlando <version>"
git push origin <release-branch>
git push origin v<version>
```

Substitute the actual version before presenting the commands. Explicitly ask the user to create the annotated tag and run the specific single-tag command `git push origin v<actual-version>`; do not say only “push the tag” and do not use `git push --tags`. Explain the two-commit boundary when it applies and require that the publishable Rust and npm trees are identical in the candidate and completion commits. If no post-publication files changed, the candidate itself is the completion commit. If GitHub Releases are used, suggest creating the release from `v<version>` and copying the matching changelog section. Do not run these Git commands unless the user explicitly authorizes them.

## Report

End with the target version, registry precheck, changelog and migration status, each unit, E2E, build, documentation, package, and dry-run result, reviewed package contents, warnings and their disposition, publication URLs, registry and clean-consumer verification, post-publication lockfile status, the artifact-provenance/tag boundary, and the exact Git/tag commands still owned by the user.
