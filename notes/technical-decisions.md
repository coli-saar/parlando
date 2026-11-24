# Technical Decisions

## 2026-11-21: Rehearse publication with local package artifacts

The crate dry run can only list a crate's contents when a new exact dependency is
absent from crates.io. Tests built from the repository also cannot show that
scaffolding, compilation, and deployment work with just the packaged files.

`make test-release-candidate` packages the six coordinated releases and runs
consumers from a temporary directory outside the checkout. Cargo patches point
to SDKs extracted from their own archives so the candidate can resolve its
exact pins before the registry has the new version. The starter clients install
the npm tarball, and a fresh virtual environment installs both SDK wheels.
Reusing Beacon Relay exercises the published scaffolding and component toolchain
for both languages without maintaining a second game. The browser checks use a
temporary loopback host and real package upload, revision selection, and intake.
Snapshot the six package trees together before building. A run can record the
tested snapshot even when an unrelated edit lands during compilation. The
publication preflight names any inputs that changed since the snapshot.

The candidate target runs the ordinary, end-to-end, and component suites,
including runtime stress tests, before it retains the rehearsal's artifacts.
The coordinated publish target checks the version, source fingerprints, and
artifact checksums. npm and Twine upload the exact retained files. Cargo's
standard publish command packages the verified source again with registry
dependency metadata and VCS provenance. This preserves Cargo authentication,
verification, and registry polling without implementing a separate upload
client. Registry dry runs and post-publication consumer checks cover that
registry boundary. Individual upload targets remain available for a partial
release, with npm and Python using the saved candidate.

## 2026-09-02: Participant admission retries reuse captured launch identity

Context: The browser deliberately removes Prolific identifiers from the visible URL before sending
the participant-registration request. If registration or a later room-startup request failed, the
join screen remained available, but a second click reread the now-clean URL and attempted a new
registration without the required identity. This replaced the original failure with a misleading
“Prolific participant parameters are required” error.

Decision: Capture the registration payload once per participant client and reuse it across
registration retries. Keep the Prolific parameters in the URL until registration succeeds and the
returned participant credential has been written and read back from tab-scoped storage. Only then
remove the recruitment identifiers from the visible URL. This ordering means a reload or component
remount before registration completes can repeat the idempotent Prolific admission instead of
losing both the launch identity and the credential. Coalesce concurrent registration calls and use
a synchronous entry guard so one click sequence cannot start two participant lifecycles. Once the
client holds a participant credential, entering the waiting room replays consent and resumes room
creation with that credential instead of registering again.

Treat room admission and game-channel connection as separate client states. As soon as the server
returns a waiting-room snapshot, render that authoritative room even if obtaining or opening the
game channel fails. Retry only channel setup from that screen; do not register the participant or
create the room again.

On reload, restore every authoritative participant state, including a terminal `ended` state, even
when no live session object exists yet. A terminal snapshot needs no WebSocket; materialize the
minimal client session container so the outcome and recruitment handoff render instead of falling
through to the join page.

Tradeoff: Recruitment identifiers remain visible during the registration request, and remain in
the URL for the page lifetime if tab storage is unavailable. This is preferable to erasing the only
recoverable launch identity before the browser possesses a reload-safe credential. Pending launch
identity also remains in the participant client's memory for the lifetime of the page.

## 2026-09-02: Each Local Preview action creates one participant invitation

Context: The dashboard retained one synthetic Prolific participant/session identity for the entire
testing run. After one preview completed, opening Local Preview again reused that identity. The
idempotent participant endpoint correctly resolved it to the earlier terminal session, so the new
browser attempt could not enter a new waiting room.

Decision: Mint fresh cryptographically random `TEST…` participant and session identifiers for every
dashboard **Open** or **Copy URL** action during a Prolific Local Preview. Treat the resulting URL as
one participant invitation: retries and reloads of that same URL remain idempotent, while a new
dashboard action represents a new preview participant. Provider-backed testing, official intake,
and Direct participant URLs are unchanged.

Tradeoff: **Open** and **Copy URL** intentionally produce different invitations when invoked
separately. A researcher who wants another browser to use a particular preview invitation must use
the URL produced by that specific Copy action.

## 2026-09-02: Prolific consent refusal has no local fallback

Context: The participant client rendered consent refusal only as a Prolific completion link. The
Great Tree `rootbot-agent` draft has an empty `no_consent` code because the earlier six-path data
conversion could not derive a researcher-created Prolific code. Local Preview intentionally allows
that incomplete provider configuration, so its refusal action disappeared even though consent was
still required.

Decision: A Prolific-enabled participant page exposes **Do not consent** only as the configured
Prolific completion link. Require all six completion codes before Local Preview can start. Do not
replace a missing provider handoff with a Parlando outcome page. Direct experiments retain their
local refusal page because they have no recruitment provider to return to.

Tradeoff: The existing `rootbot-agent` testing run must be stopped before its inactive configuration
can be repaired with the researcher-created No consent code. The server cannot infer or generate a
Prolific completion code.

## 2026-09-02: Active experiment configuration is a server-side invariant

Context: The dashboard disabled configuration controls outside the inactive lifecycle, but that UI
rule is not an authority boundary. Runtime construction also captures an immutable configuration
revision, so permitting a configuration write during intake could make one experiment span two
in-memory revisions.

Decision: Reject configuration writes unless the stored experiment is inactive. Check this in the
HTTP handler for a precise conflict response and retain the transactional SQLite condition on both
the expected revision and `status = 'inactive'` as the authoritative race-safe invariant. Serialize
configuration saves and runtime construction for the same experiment, then evict its cached runtime
after a successful save. Apply the same cache discipline to archival, transition to inactive, and
shared provider-setting changes affecting inactive runtimes.

Tradeoff: An administrator must stop intake before changing configuration, even when a proposed
field appears operationally harmless. This is deliberate: Parlando has one immutable configuration
revision per intake run and no field-level live-reconfiguration contract.

## 2026-09-02: Experiment dispatch uses explicit namespaces and routes

Context: Multi-experiment hosting reused a single-experiment router by rewriting each incoming URI,
clearing all Axum request extensions, selectively restoring known extensions, and invoking the
child router. This coupled dispatch to Axum's private path-capture representation and could silently
discard future request context. This decision supersedes the 2026-08-15 transport-state workaround
described later in this file.

Decision: Build each cached participant and administrator router with its complete literal
`/e/{experiment_id}` or `/api/admin/runtime/{experiment_id}` prefix. A single installation-router
fallback accepts only those two namespaces, resolves the experiment ID, and forwards the original
request without changing its URI or extensions. Every other unmatched path immediately returns
404. Use a per-experiment construction lock so concurrent first requests create one runtime.

The fallback is an Axum integration boundary, not route behavior. Experiment IDs are created at
runtime, while `build_game_router` must continue returning Axum's public `Router` type. Registering
outer wildcard routes and then forwarding into a cached Axum router leaves outer `MatchedPath` and
URL-parameter metadata on the request: the second router either rejects that request or exposes
duplicate path parameters to handlers. Clearing extensions or rewriting the URI would recreate the
request-mutation hack this change removes. Axum invokes a fallback without route-local match
metadata, so it is the only request-preserving dynamic-router adapter available under that public
interface. Replacing it cleanly would require changing the public server type or refactoring every
runtime handler to resolve dynamic state directly from an installation-level router.

The participant HTTP surface is deliberately closed: `/e/{experiment_id}` redirects to its
canonical trailing-slash URL, `/e/{experiment_id}/` serves the participant shell,
`/e/{experiment_id}/assets/...` serves built assets, and named HTTP and WebSocket endpoints serve
the runtime protocol. Unknown participant, administrator, and top-level paths return 404. There is
no static-file or SPA fallback because the participant client does not use browser-side path
routing.

Tradeoff: Experiment routers still provide an internal Axum service boundary, which keeps existing
handler state extraction, the public Axum router type, and all public routes unchanged. The narrowly
validated host fallback is unavoidable under those constraints, but it cannot turn an unknown path
into successful content. A mistyped participant URL receives 404 instead of the participant shell;
this is intentional fail-closed behavior.

## 2026-09-02: Software agents join an already-forming human session

Context: Human-agent admission previously awaited agent factory construction while holding the
experiment-wide admission lock. A slow external agent constructor delayed the human response and
blocked unrelated admissions, while the participant never observed the real waiting phase.

Decision: Create and persist the forming session with the human under the short admission lock,
then release the lock and return its waiting state. Construct the reserved role-B agent in a task.
On success, persist and attach the agent participant, broadcast the updated presence, and invoke the
same readiness transition used by ordinary participant connections. On construction failure, end
the forming session with a technical-failure result. A human departure causes a late agent result
to be rejected and shut down rather than attached.

Tradeoff: Game construction and initial-state creation occur when the forming session is created,
before the asynchronous agent factory completes. The agent receives no observation and cannot act
until it has joined and common readiness starts the game. Agent-participant identity may be created
before a failing external constructor returns; normal unattached-participant cleanup removes its
ephemeral runtime session.

## 2026-09-02: The administrator dashboard uses embedded static modules

Context: One HTML source combined markup, CSS, and roughly 2,400 lines of JavaScript. A large Rust
test asserted hundreds of source substrings, coupling harmless presentation refactors to the server
test suite without exercising browser behavior.

Decision: Keep semantic markup in `admin_dashboard.html`, move presentation to an embedded CSS
asset, and serve JavaScript as same-origin ES modules with separate state, administrator-request,
and pure-formatting boundaries. Preserve single-binary deployment through `include_str!` and protected Axum asset
routes. Replace the source-spelling test with route, content-type, CSP, and non-empty asset checks,
plus executable unit tests for pure formatting and escaping behavior.

Tradeoff: The main feature controller remains sizable and should be split further by dashboard
area as those areas change. Native ES modules add a few same-origin requests, but avoid a Node build
dependency and make independent behavior tests possible.

## 2026-08-31: Runtime stress keeps every game transport live explicitly

Context: The release-mode human-human stress profile consistently lost one relayed frame at five
seconds. The preserved database showed that role B's game connection had already been closed by the
production liveness policy at game time 5.002 seconds. The harness sent role A's messages and
actions but left role B completely silent, so the later audio deadline was a secondary symptom of a
correct participant-disconnect transition rather than relay corruption.

Decision: Send the ordinary participant-protocol heartbeat once per second on both game WebSockets
during the measured workload. Heartbeats preserve transport liveness but do not count as meaningful
research activity, matching the browser client's production behavior and the Prolific idle tests.

Tradeoff: The stress fixture now generates a small amount of additional game-channel traffic. This
is representative browser overhead and keeps audio-relay measurements from being invalidated by an
unrelated silent-transport timeout.

## 2026-08-31: The crate ships one public voice configuration example

Context: Package inspection found both the maintained full voice configuration example and an
unreferenced `experiment.voice.private.example.yaml` overlay. The latter contained placeholders,
not credentials, but duplicated the public example and put a private-overlay filename in the
published artifact.

Decision: Remove the unused private-overlay template and publish only
`experiment.voice.full.example.yaml`. Actual private configuration remains excluded by the Cargo
manifest and credentials continue to be supplied through protected game settings.

Tradeoff: Anyone relying on the unreferenced template must use the fuller documented example, which
describes the same voice, transcription, and TTS fields without implying that a private file belongs
in the crate.

## 2026-08-31: Published Rust crates own their protobuf build inputs

Context: Cargo could create the 0.4.0 crate archive but could not verify it because `build.rs`
compiled remote-agent and learner schemas from the repository-level `proto/` directory. Files above
the Rust package root are not present when crates.io consumers build the extracted archive.

Decision: Make `rust-server/proto/` the single source for the agent v5 and learner v1 schemas. The
Rust build reads package-local paths, and the Python binding generator and protocol-source tests
read those same files. Keep browser-only wire fixtures in the repository-level `proto/` directory.

Tradeoff: Python SDK development now reaches into the Rust package directory for the shared RPC
contract. This coupling is intentional: the published Rust crate owns the wire protocols it embeds,
and every generated client must follow that exact source. The package tarball is now independently
buildable without duplicating schemas.

## 2026-08-31: The browser release gate exercises participant-visible terminal contracts

Context: The 0.4.0 release candidate passed the Rust and browser behavior suites but missed the
browser package's 80% statement and branch coverage gate. The uncovered code was concentrated in
the standard participant application's server-message projection, consent surface, terminal
outcome explanations, and Prolific completion handoff.

Decision: Cover those public behaviors through the injected `ParticipantAppTestHarness`. Tests now
drive authoritative WebSocket messages through the real decoder and reducer, exercise required
consent and participant-information links, render every terminal outcome, and verify the Prolific
completion-code copy path. Keep the 80% thresholds unchanged.

Tradeoff: The render suite is larger and repeats the normal join sequence for each terminal outcome,
but it validates the participant-visible contract at the component boundary and catches omissions
that isolated helper tests would miss.

## 2026-08-31: Defer first-party npm consumer lockfiles until 0.4.0 exists

Context: Space Game and Great Tree consume the public browser SDK from npm. Their manifests must
declare the coordinated 0.4.0 release, but npm cannot produce truthful registry resolutions,
tarball URLs, or integrity hashes before that immutable version is published.

Decision: Update both consumer manifests to `^0.4.0` during release preparation and leave their
existing 0.3.0 resolved lockfile entries untouched. Refresh those lockfiles with npm after the
0.4.0 package is visible in the registry, then verify each resolved package manifest reports
0.4.0. Do not hand-edit resolved URLs or integrity metadata.

Tradeoff: The consumer manifests and lockfiles intentionally disagree during the short interval
between candidate preparation and registry publication. Local release verification uses the
repository packages, while published-dependency verification and lockfile refresh are deferred
until npm can supply authoritative metadata.

Completion: After npm published 0.4.0, both consumer lockfiles were refreshed from the registry.
They now record the 0.4.0 tarball and its registry-provided integrity digest rather than the
temporary 0.3.0 resolution or a local workspace link.

## 2026-08-31: Parlando releases use a resumable project-local skill

Context: A coordinated Parlando release must keep the Rust crate, JavaScript package, example
consumers, migration guidance, publishing documentation, and game-generation skill on one version.
The registry uploads are immutable, while this repository forbids agents from running Git commands
without separate explicit authorization. Preparing the release also makes the checkout dirty, which
must not be bypassed with Cargo's `--allow-dirty` option for a real publication.

Decision: Add `skills/release-parlando` as the reusable release workflow. It separates preparation,
verification, and publication into resumable phases; performs a deterministic, read-only metadata
audit; runs the full tests, release builds, package inspections, and registry dry runs; and publishes
only when actual publication was explicitly requested. The workflow stops after preparation so the
user can review and commit the candidate, then resumes by rechecking the committed candidate before
publishing Rust followed by JavaScript. Git tagging and GitHub pushes remain an explicit user handoff.

Tradeoffs: The mandatory commit boundary means a release normally spans at least two skill
invocations. This prevents an immutable artifact from being built from uncommitted state and makes
the tag correspond to the reviewed candidate. The metadata script intentionally checks first-party
release contracts rather than replacing every old version string, so historical migration material,
test fixtures, and transitive dependency versions remain intact.

## 2026-08-31: Saving Prolific drafts is independent of provider readiness

Context: The dashboard stored the installation-wide Prolific API token correctly but omitted it
from the configured-provider status response, so a refresh falsely showed the credential as
missing. Once that token existed, saving a complete-looking experiment draft also ran the remote
Prolific preflight before writing the new revision. Any provider readiness issue rejected the whole
request, so edits such as the new `no_consent` code disappeared even though they were structurally
valid and needed to be retained for correction.

Decision: Report `prolific.api_token` alongside the other write-only provider credentials. For an
inactive experiment, commit a structurally valid configuration revision atomically before running
the Prolific preflight. Return and cache the provider issues as readiness information; they may
block provider-backed launch, but they do not roll back or conceal the saved draft. Local schema,
game-configuration, secret, and optimistic-concurrency failures still reject the save because those
would not produce a coherent revision.

Tradeoff: A saved revision can be provider-unready, which is already supported for incomplete
Prolific drafts and Local Preview. Researchers can now inspect and repair the exact saved inputs,
while Test through Prolific and Official intake remain guarded by the same remote validation.

## 2026-08-31: Prolific routing projects the terminal state machine

Context: The six-path Prolific design described completion selection as a pure mapping from
`ParticipantOutcomeKind`. That is insufficient for `connection_lost`: the same factual participant
outcome can result from reconnect expiry while the shared session is still `Forming` or after it is
`Running`. Mapping both cases to `participation_ended_early` contradicts the promise that this path
is used only after a playable game began.

Decision: Treat Prolific completion selection as an output of the canonical participant and session
state machines, not as another lifecycle. The projector receives the immutable participant result
and the durable fact that the session did or did not start. Technical and lifetime outcomes map to
`technical_failure`; normal completion maps to `completed`; a peer departure or peer reconnect
expiry maps to `partner_left`; another own-participation outcome maps to `game_did_not_start` before
session start and `participation_ended_early` after session start. `no_consent` remains a direct
pre-registration route and creates no Parlando participant state. The corrected correspondence and
diagram are recorded in `notes/prolific-completion-paths-design.md` and reuse the state machine in
`notes/participant-state-machine-design.md`.

Tradeoff: The projector needs one durable session-phase fact in addition to the participant outcome.
This avoids adding phase-specific outcome variants, preserves the exact `connection_lost` fact, and
keeps the provider categories aligned with whether play actually began. Tests must cover reconnect
expiry from both `Waiting`/`Forming` and `Paused`/`Running`.

Populated workspace conversion: After confirming that no process held the database open, backed up
`games/great-tree/server/parlando-great-tree.sqlite` as
`games/great-tree/server/parlando-great-tree.schema16-pre-prolific-six-path-backup-2026-08-31.sqlite`.
In one immediate transaction, renamed stored `partner_unavailable` configuration values to
`game_did_not_start`, renamed `timed_out` to `participation_ended_early`, and added an empty
`no_consent` value to the affected current configuration and historical revisions. The existing
codes `GREENTIGER` and `RAPIDPANDA` were preserved; no historical No consent code could be derived.
The `rootbot-agent` experiment therefore remains locally previewable but is not provider-ready until
its researcher supplies and verifies that sixth code. Schema version 16 was unchanged because only
versioned JSON values changed. Validation retained 4 experiments, 17 configuration revisions, 50
participants, and 42 sessions; integrity was `ok`, the foreign-key check returned no rows, and no
old completion-field key remained in current or historical configuration.

No-consent redirects use the configured `no_consent` completion code in every active intake mode,
including Local Preview. Parlando emits the normal participant completion URL only when that code
is nonempty; an incomplete draft suppresses the link instead of constructing an empty `cc=` query.
This preserves realistic local testing without accidentally handing Prolific an unspecified
completion path.

## 2026-08-28: Prolific uses six coarse completion paths while Parlando retains precise outcomes

Context: Prolific completion paths trigger provider processing, whereas Parlando records the
specific session cause and participant consequence. Treating the Prolific label as the complete
factual account either multiplied paths that had identical actions or made umbrella labels such as
`timed_out` appear more precise than the evidence. The existing five-path configuration also had no
dedicated route for a participant who declined consent.

Decision: require six Prolific completion fields for provider-backed testing and official intake:
`completed`, `partner_left`,
`game_did_not_start`, `participation_ended_early`, `technical_failure`, and `no_consent`.
`partner_left` is the Prolific path and label both when the partner deliberately leaves and when the
partner's connection does not return; Parlando retains and presents **Partner chose to leave** or
**Partner connection lost** as the precise outcome. Use automatic approval for `completed`,
`partner_left`, and `technical_failure`; request a return for `game_did_not_start`,
`participation_ended_early`, and `no_consent`. Missing or invalid paths have no fallback. Declining
consent redirects through Prolific before Parlando registration and creates no participant or
session data.

Local Preview is a provider-independent dry run of the experiment. An incomplete Prolific setup is
therefore saveable and previewable, but it keeps **Test through Prolific** and **Official intake**
unready. Researchers make partial payments for `game_did_not_start` by hand through Prolific bonus
payments; Parlando supplies the relevant unsuccessful waiting time but does not track completion of
the return or bonus-payment workflow.

Keep the dashboard's existing disclosure hierarchy. The stable session-fact grid remains visible,
exceptional or terminal participant outcomes remain below the participant name, and provider facts
remain behind the Prolific-only badge aligned with that name. Do not add an always-visible
completion-path or reconciliation block. The same Parlando client opened from the Prolific study
shows the precise terminal result and then the provider handoff.

Validate the linked Prolific study remotely only when saving a Prolific-enabled experiment or when
an administrator explicitly selects **Recheck**. Persist the successful result against the exact
experiment revision and a fingerprint of all relevant local configuration, provider-endpoint, and
credential inputs. Dashboard polling reads that durable result and never contacts Prolific. Start
requires the same matching result that made its mode green and does not perform another remote
preflight. Any relevant local change invalidates the result immediately; a study changed only in
Prolific remains verified until explicit Recheck or the next Save. Per-participant submission
verification remains an admission operation and does not repeat study-configuration validation.

Do not hold a database transaction open during provider validation. Save reads the expected
experiment and Game Settings revisions, performs any provider request outside the transaction, and
then uses a short transaction that rechecks both revisions before storing the new immutable
experiment revision and its verification result. A concurrent edit yields a revision conflict.
Canonical study URLs come from the existing `Server::public_url`; Parlando checks only that the
retrieved study uses URL parameters and points to that experiment's canonical `/e/{experiment_id}/`
landing URL. Prolific remains responsible for general URL validation.

Non-goals: Parlando does not add completion paths for unobserved pre-game exits, implement
post-game consent withdrawal, automate or track partial bonuses, use participant groups as a
payment queue, track whether requested returns were completed, continuously revalidate studies,
add another public-URL setting, retain a runtime compatibility schema, or automatically replace
Prolific places consumed by approved incomplete sessions. These boundaries are part of the accepted
design, not missing fallbacks. The full negative contract is recorded in
`notes/prolific-completion-paths-design.md`.

Tradeoffs: Prolific deliberately receives a coarser category than Parlando stores, so its
**Partner left** label does not distinguish intent from connection failure. This avoids separate
codes with identical compensation behavior while preserving the distinction in Parlando's durable
data, participant terminal explanation, dashboard, and exports. The sixth code adds one setup step
but implements Prolific's prescribed no-consent return path without storing data for someone who
did not consent. Persisted validation does not detect provider-only edits automatically, but avoids
slow provider calls during dashboard refresh and ensures that a green Start mode uses exactly the
contract already shown as ready. The full design and implementation plan are recorded in
`notes/prolific-completion-paths-design.md`.

## 2026-08-28: Derive each Prolific workspace from its linked study

Context: Parlando already links one Prolific study at the experiment level. Requiring a second,
installation-level workspace ID duplicated information available through the study's project and
incorrectly suggested that connecting a workspace also linked a study. An API token may also grant
access to studies in more than one workspace.

Decision: Keep only the Prolific API token and API endpoint in global Game Settings. When a
Prolific-enabled experiment configuration is saved, retrieve its exact study and project and derive
the workspace ID from that project. Retain the derived ID with the provider-verified study for the
open process and compare it with the workspace claim in a Secure external URL token. A missing
project or an unreadable project makes the experiment configuration invalid. The dashboard no
longer asks for or displays a global workspace binding.

Tradeoffs: Rotating a token no longer requires repeating a workspace ID, and one game process can
host experiments from different workspaces accessible to that token. Study validation now supplies
the workspace trust boundary, so Prolific-backed intake cannot start without a successful study and
project preflight. The workspace title is no longer retained because it has no operational role.

Populated workspace conversion: After verifying that no process listened on port 8080 or held the
database open, backed up `games/great-tree/server/parlando-great-tree.sqlite` as
`games/great-tree/server/parlando-great-tree.schema15-backup-2026-08-28.sqlite`. Transactionally
removed `prolific_workspace_id` and `prolific_workspace_title` from `game_settings` and advanced the
schema from 15 to 16. The global ID and verified title have no correspondence in the derived model;
all remaining settings and all experiment, participant, session, assignment, and Prolific
submission rows were preserved. Before and after conversion there were 4 experiments, 50
participants, 42 sessions, 84 session assignments, 11 Prolific submissions, and one Game Settings
row. Post-conversion integrity was `ok`, the foreign-key check returned no rows, no workspace
columns remained, and all required provider endpoint settings were populated.

## 2026-08-27: Game Settings retain drafts and organize provider bindings together

Context: the five-second dashboard refresh replaced Game Settings controls with the last committed
server values. An unsaved Prolific workspace ID therefore disappeared without feedback, and the
Prolific token was separated from its workspace and endpoint under a generic hosted-services
section. Experiment readiness also checked Speechmatics, ElevenLabs, and agent dependencies but
omitted the installation-owned Prolific token and workspace.

Decision: treat edits to any shared setting or provider credential as one revisioned local draft.
Periodic catalogue refreshes may update server state but do not overwrite that draft. Save errors
remain inline beside the single save action, and the draft remains available for correction. Split
provider settings into Speechmatics, ElevenLabs, and Prolific sections so each endpoint and secret
are colocated; label the action “Save all game settings” and state that it commits every section in
one transaction. The initial implementation also verified a global Prolific workspace ID; the
2026-08-28 decision above removed that redundant binding and derives the workspace from each linked
study instead.

Calculate two readiness sets: local preview checks structural configuration and required game
services, while Prolific testing and official Prolific intake additionally check the global token
plus the exact experiment-owned Prolific study contract. Each Prolific-enabled
experiment links to one study; connecting a workspace does not link a study. Present icon-only
readiness badges beside each launch mode instead of assigning one readiness judgment to the whole
experiment. Direct experiments show only Local preview and Official intake, with no Prolific
terminology. Structural configuration errors and missing runtime or provider prerequisites remain
distinct in each badge tooltip.

Expose the literal Prolific-placeholder URL as setup information in every inactive experiment's
Prolific configuration, before the experiment is runnable. The researcher uses it to create the
one corresponding Prolific study, then enters the resulting study ID and completion codes in
Parlando. Saving a Prolific-enabled configuration performs a fresh provider preflight and rejects
the revision if the study cannot be retrieved or does not satisfy the full contract. This removes
the circular dependency between obtaining the external URL and linking the study while making the
verified study part of experiment validity.

Collapse inactive launch modes into one “Start experiment” dropdown so readiness choices do not
compete with Clone and Archive in the header. Clone is available only while inactive. Keep exactly
the actions needed during an open testing run directly visible: Open, Copy URL, and Stop. Direct
testing uses the ordinary participant URL. Treat the participant URL as state of the open intake
run: choose it when testing or official intake starts, return the same value to every dashboard
catalogue refresh, and clear it when intake ends. Open and Copy URL both use this one retained value.
A Prolific Local Preview retains generated `TEST…` participant and session IDs; testing through
Prolific and official Prolific intake retain the literal provider-placeholder form; Direct intake
retains the query-free URL. The URL state is process-owned rather than database-backed because
startup already deactivates every intake run left open by an earlier process, so there is no running
URL to restore across a server restart. This preserves frequent actions as one-click controls while
containing the less frequent mode choice. Periodic dashboard refreshes preserve an open launch menu
when the experiment identifier and lifecycle are unchanged; an experiment switch or lifecycle
transition closes it.

Use one delegated dashboard tooltip for elements carrying a `title`, converting the native title on
first hover or keyboard focus and displaying it after 90 milliseconds. Delegation covers controls
created by periodic rendering without per-control setup, while hiding on pointer exit, focus exit,
click, scrolling, or resizing. This avoids the browser-controlled native tooltip delay without
changing the concise icon-first layouts.

Keep both local and provider-backed testing under the durable `testing` lifecycle and immutable
testing data purpose. Dashboard-generated alphanumeric participant and session IDs beginning with
`TEST` may bypass external Prolific verification only in that lifecycle and are recorded with the
`synthetic_testing` verification method. A real Prolific test performs the ordinary provider
preflight as a transient action without creating another lifecycle state. Testing must stop before
official intake starts; the testing-to-active transition is rejected by the server as well as omitted
from the dashboard.

Tradeoffs and risks: a draft begun against an older Game Settings revision deliberately receives a
revision conflict if another administrator saves first. Provider-backed badges and Start use the
same full remote-study preflight. Cache its result for 30 seconds by a fingerprint of every local
preflight input, including the protected token, so five-second catalogue polling does not repeatedly
contact Prolific and any configuration or credential change invalidates the result immediately.
External provider changes may take at most 30 seconds to appear in a badge. The conspicuous `TEST`
prefix is both the visual distinction in session details and the server-side boundary for the
testing-only verification bypass.

## 2026-08-27: Repair rootbot-agent revision 8 with a clean revision 9

Context: `rootbot-agent` revision 8 contained both the valid top-level `game: {}` value and a stray
`recruitment.prolific.completion_paths.game: {}` value. The completion-path type denies unknown
fields, so the experiment configuration could no longer be parsed or repaired through the curated
dashboard form. The bad property was already present in the schema-12 backup from 24 August and
first appeared in revision 8; available revision records do not identify the submitting client.

Decision: after confirming that no process held the database open, create the consistent backup
`games/great-tree/server/parlando-great-tree.pre-rootbot-config-repair-2026-08-27.sqlite`. Preserve
revision 8 unchanged and transactionally create revision 9 by removing only the nested
`completion_paths.game` property. Keep the valid top-level game configuration and the five Prolific
completion codes unchanged. Record the correction as `Removed stray game property from Prolific
completion paths` in the immutable revision history.

Validation: the live database remains at schema 15; `pragma integrity_check` returned `ok`, the
foreign-key check returned no rows, and revision 9 is valid JSON with a top-level game object and no
nested completion-path game property. Removing that one path from revision 8 produces byte-for-byte
the revision-9 JSON. Experiment, session, participant, and event counts remained 4, 41, 49, and
1230; the revision count increased from 16 to 17. The backup passed its own integrity check and
retains rootbot-agent revision 8 as the recovery point.

## 2026-08-27: Session summaries show facts only when they explain state or exceptions

Context: every selected session displayed its waiting start and deadline, an empty unsuccessful-wait
field, routine health, research purpose, direct recruitment, pending end cause, and last-activity
time. Participant cards likewise repeated ordinary transport health and expanded Prolific metadata.
The resulting summary gave normal sessions more operational metadata than useful state.

Decision: keep a stable labeled fact grid for session state, health, research/testing purpose, and
Direct/Prolific recruitment so categorical dots never lose their meaning. Put the dialogue name
beside the numeric session identifier. Show waiting timestamps only while a session is forming and
elapsed wait only for a terminal unmatched session; add end reason only after termination and
diagnostic activity time only for abnormal running health. Place participant phase or terminal
outcome on a separate line below the participant name. For Prolific participants only, put a green
check or gray question icon immediately after the name to indicate whether provider status was
checked; its tooltip explains the state, the icon opens the collapsed provider details, and that
detail explicitly says “Has been checked” or “Has not been checked.” Omit the
reconciliation timestamp because the icon communicates the useful checked/not-checked distinction.

Termination labels use the shared session cause to disambiguate recipient outcomes. An explicit
leave action is labeled as explicit; reconnect expiry is labeled as connection loss, with a tooltip
stating that the server cannot distinguish a closed browser tab from a network interruption. The
partner's badge inherits that same distinction instead of calling both cases "partner left."
Reconnect timing is displayed only while the participant is disconnected and the deadline is still
in the future. After termination, the durable cause and participant outcome replace obsolete
deadline metadata.

Tradeoffs and risks: the summary is no longer a complete inventory of stored timestamps. Four
stable session facts take more room than an unlabeled badge row, but remain interpretable without
hovering or memorizing dot colors. Detailed provider identity remains one click away and is never
rendered for Direct participants; full event history and exported data are unchanged. Conditional
timing still depends on canonical lifecycle, participant state, health, and terminal outcomes rather
than inferring urgency from timestamps alone.

## 2026-08-27: Active Prolific participant links expose provider placeholders

Context: the dashboard's participant link added locally generated participant and session IDs for
every Prolific-enabled experiment. Those values are useful while testing intake, but an active
experiment needs an external study URL that Prolific can specialize for each real submission.

Decision: retain stable synthetic IDs for test links and render Prolific's literal
`{{%PROLIFIC_PID%}}`, `{{%STUDY_ID%}}`, and `{{%SESSION_ID%}}` placeholders when the experiment is
active. Construct that query text literally so browser URL serialization does not percent-encode
the placeholder syntax. Leave all non-Prolific participant links unchanged.

Tradeoffs and risks: opening an active link directly does not represent a valid participant launch;
the link is a configuration value to copy into Prolific. The dashboard title communicates that
distinction. Production admission remains subject to the existing Prolific verification path.

## 2026-08-26: Linux game packages use one convention-driven cross-build script

Context: macOS development needs to produce a complete game deployment for a Linux host that has
no Docker daemon. Each current game already places its Rust package at `server/Cargo.toml` and its
browser application at `client/package.json`, while Cargo packages can contain auxiliary binaries
that must not be deployed as the game server.

Decision: add one repository-level `scripts/package-linux.sh` command and run it from a game root.
Use `cargo metadata` rather than shell parsing to select `package.default-run`, a package-named
binary, or the only binary in that order. Infer the browser project from the shared game layout.
Use the published Rust and JavaScript dependencies from the game manifests by default; an explicit
`--local` option applies both sibling overrides together, matching Great Tree's run command.
Cross-compile a static MUSL binary with `cargo-zigbuild`, and publish a stable
`.local/package/<binary>-<architecture>-linux` directory containing the binary, browser assets,
relocatable launcher, build facts, and checksums. Default to x86-64 Linux and permit an explicit
ARM64 target. Replace the stable local package only after every build and verification succeeds.
Use the repository-local npm cache already established by the Makefiles so packaging is isolated
from ownership or content problems in a developer's global npm cache.
Keep cargo-zigbuild's generated linker wrappers and Zig's global and local artifacts in
repository-local caches for the same reason.
Build the game client in a temporary source copy. In local mode its SDK dependency points to the
freshly packed sibling SDK. This keeps the local deployment's Rust and JavaScript runtime versions
coordinated without rewriting the game's committed manifest or lockfile as a packaging side effect.
In published mode, explicitly install the SDK requirement declared by the game after ordinary npm
resolution. This recovers from a previous local run having recorded a sibling SDK in the lockfile
without silently turning a nominally published package into a local one.

Tradeoffs and risks: a convention-driven script rejects games with nonstandard server or client
layouts rather than adding per-game configuration. A stable directory makes `rsync --delete`
simple but is not an atomic remote deployment, so the service must be stopped during transfer.
The static binary still reads the host CA-certificate bundle for outbound TLS. Package generation
changes ignored build outputs such as `node_modules`, `dist`, Cargo targets, and `.local`; it does
not include a database, provider credentials, or other mutable deployment state.
The supported user workflow and package contract are documented separately in
`docs/cross-compiling-for-linux.md`; the broader deployment guide links to that task-specific guide.

## 2026-08-26: Remote-agent configuration exposes only endpoint and YAML settings

Context: the standard remote factory exposed opaque JSON, endpoint, agent name, agent version,
protocol version, a transport bearer credential, and an agent-instance secret in the experiment
dashboard. JSON rendered as `[object Object]`, optional secret references were materialized even
when unset, and the Python service ignored the user-supplied name and version. Protocol and
transport-authentication details were mixed with agent-owned experiment settings.

Decision: replace the remote settings shape with `endpoint` plus an optional `config_yaml` string.
The dashboard renders YAML as a multiline editor with an empty default; Rust requires an empty
document or mapping and sends the parsed mapping as the protocol `config`. Protocol v5 removes
protocol, name, version, and secret fields from `CreateAgent`. Remote processes own model/provider
credentials. Both the Rust adapter and Python SDK automatically read the deployment-level
`PARLANDO_REMOTE_AGENT_TOKEN`; it never enters an experiment record or dashboard response. Keep
the existing HTTPS and explicit host-allowlist requirements for non-loopback endpoints.

Tradeoffs and risks: remote participant provenance now identifies the standard transport as
`remote-agent@parlando-agent-v5` plus the exact endpoint/YAML settings fingerprint; it
does not claim to identify mutable code or model weights behind an endpoint. Remote deployments
must arrange the same transport token in both process environments. The v5 protocol deliberately
has no compatibility reader for v4. Neither populated workspace database contained a
`remote_grpc` experiment configuration, so no live data conversion or backup was required; the
open Great Tree database was left untouched.

## 2026-08-24: Every game server exposes the remote gRPC agent

Context: the dashboard derives its agent choices from factories registered by each game binary.
Remote gRPC support therefore appeared only when a binary explicitly registered `RemoteAgent`,
even though the transport and its configuration schema are part of the Parlando server.

Decision: add the standard remote gRPC factory when `Server::serve` finalizes the registered
factory set. Preserve an explicitly registered `remote_grpc` factory and add no duplicate. The
dashboard and runtime continue to consume the same factory set, so every displayed choice is also
instantiable. Game binaries register only their additional in-process factories.

Tradeoffs and risks: every dashboard now exposes remote endpoint, identity, and credential fields,
including deployments that use only in-process agents. Selecting the option still subjects the
endpoint to the existing transport, allowlist, authentication, and activation validation. The
default is injected at the public server boundary; lower-level internal router test hooks retain
explicit factory control.

## 2026-08-24: The dashboard projects canonical lifecycle and participant phase directly

Context: after the participant-state redesign, durable session responses exposed `lifecycle` with
`forming | running | ended`, but the dashboard still read `status`, filtered by the removed
`waiting | completed | abandoned | expired` vocabulary, and looked for obsolete participant
`outcome` fields. A running session therefore rendered as `Unknown`, and participant phases were
not visible. Durable participant connection strings also could not represent a live pause.

Decision: use `lifecycle` throughout the administrator session API, query parameters, telemetry,
filters, and rendering. Display the structured session-end cause separately from lifecycle and
transport health. Project each attached participant as `waiting`, `active`, `paused`, or `ended`
from the authoritative live session snapshot; attach recipient-specific terminal results from
durable storage. Publish the same live participant phase in the five-second operational snapshot
so an already-selected dashboard session follows pauses and reconnections without conflating them
with socket health. When a nonterminal durable session is absent from the current runtime, render
its participant phase as unavailable rather than inventing `active` or `paused`.

Tradeoffs and risks: the administrator participant projection intentionally exposes the canonical
phase and terminal result, not role observations or available actions. Those gameplay payloads are
unnecessary for monitoring and remain on the participant protocol. Runtime-absent nonterminal
sessions expose an honest availability gap; restart recovery or forced termination remains a
separate lifecycle policy. Tests cover the complete session inventory, the attached participant
inventory, invalid lifecycle filters, and live pause/reconnect changes through real WebSockets and
administrator HTTP requests.

## 2026-08-24: Existing Great Tree data is converted once, outside the runtime

Context: the Great Tree catalogue at `games/great-tree/server/parlando-great-tree.sqlite` was a
populated schema-12 database. The schema-13 participant-state implementation intentionally has no
runtime migration, fallback decoder, or dual representation, so the server correctly refused to
open that file.

Decision: create a consistent SQLite backup beside the catalogue, then convert the live database
in place in one immediate transaction. Rename the session lifecycle and terminal timestamp columns,
add initialization and structured terminal-value columns, derive typed session ends and
recipient-specific participant results from the existing completion values and terminal events,
remove the superseded outcome columns, and stamp schema 13. Preserve running sessions as running.
The application remains schema-13-only; this is an operational data conversion, not application
migration code.

Tradeoffs and risks: historical terminal records do not contain final role observations or
recruitment handoffs, so those optional result fields remain absent. Waiting timeout maps to
`PartnerUnavailable`; running idle timeout maps to `IdleTimedOut`; recorded participant departures
preserve their actor role. The backup
`games/great-tree/server/parlando-great-tree.schema12-backup-2026-08-24.sqlite` is the recovery point.
SQLite integrity, foreign keys, terminal JSON validity, and terminal-result completeness were
checked after conversion.

## 2026-08-21: Sessions own game instances and extension loggers

Context: Parlando's live runtime stores one shared game value per experiment and one
`GameRoom` containing only state per play-through. That prevents arbitrary game helpers from
retaining a session logger, while the room/session distinction names the same one-to-one
play-through twice. The agent-agent runner design similarly gives each run session-local agents
and state but retains a shared `Arc<G>`.

Decision: use session as the common domain concept, rename the live process-local representation
to `LiveSession`, and create one game value, agent instance per automated role, and logical
`SessionLogger` per session. Reusable `GameFactory` and `AgentFactory` values construct those
session-local objects. The game and agent retain runtime-scoped logger handles, so arbitrary
implementation helpers can log strings without logger parameters on gameplay methods. Live log
handles write bounded `extension_log` events through SQLite; headless execution supplies the same
logger capability without using the live database. Keep live and headless orchestration separate
rather than introducing a general state-owning `Session<G>` engine.

Tradeoffs: live session creation must become durable before game and agent construction, so
initialization failures become recorded terminal sessions rather than disappearing before a
session row exists. Synchronous logging acknowledges bounded queue acceptance rather than an
immediate durable commit, and a crash can lose the unflushed tail. Arbitrary text expands the
privacy and storage-abuse boundary, requiring explicit byte limits, disclosure, deletion tests,
and continued exclusion from fixed corpus exports unless deliberately added. Remote agents need
a protocol extension because an in-process logger handle cannot cross gRPC.

Follow-up risks: evaluate the initial byte and five-second drain limits, specify headless
log-artifact configuration, and add remote log sequencing if transport retries are introduced. The complete
design and comparison with the previous live and runner models are recorded in
`notes/session-runtime-logging-design.md`.

## 2026-08-20: Runtime stress retains the ordinary agent deadline

Context: an initial attempt to make a 100-room run pass raised the fixture's agent deadline from
two to thirty seconds. That would make a green result less predictive: the test would no longer
exercise the same deadline that a normal Parlando human-agent experiment receives.

Decision: remove the fixture-specific agent timeout and retain `HumanVsAgentConfig`'s ordinary
default of ten seconds. Treat startup failures as a workload observation, and distinguish a
connection burst's admission rate from sustained concurrent-session capacity rather than hiding
them by extending configuration limits.

Tradeoffs: high-rate admission can fail even when an already-established workload would be
healthy. Reports and dashboard design must expose that distinction instead of converting one
measurement into the other.

## 2026-08-20: Runtime stress publishes a monotonic harness revision

Context: interactive stress investigations need to distinguish a currently running binary from
an earlier build without inferring that fact from Cargo's short compilation summary or file
timestamps.

Decision: give the runtime-stress harness a manually incremented revision number and expose it
in the terminal banner, final terminal summary, and serialized run configuration. Increment the
number whenever the harness behavior changes.

Tradeoffs: the revision is intentionally a small manual release marker rather than a general
build provenance system. It makes local iteration unambiguous with no build-script or Git
dependency.

## 2026-08-20: Runtime stress renders its TUI outside Tokio's scheduler

Context: the 100-session workload completed cleanly in headless mode but repeatedly stalled at
59 started sessions when its Ratatui dashboard was visible. The previous implementation called
the synchronous terminal event/render loop directly from the async `main` function, occupying a
Tokio runtime worker during the most concurrent part of HTTP, WebSocket, SQLite, ASR, and agent
startup.

Decision: run the terminal dashboard on its own operating-system thread and await that thread
through Tokio's blocking pool. The workload remains entirely on Tokio's scheduler; quitting the
dashboard still sets the existing cancellation flag and produces a clean shutdown.

Tradeoffs: the tool uses one additional thread only when the interactive dashboard is enabled.
This isolates rendering from measured runtime behavior, which is more important than minimizing
developer-tool thread count.

## 2026-08-20: Live stress status reflects accumulated task failures

Context: the runtime stress dashboard colored its `Failures` counter red during a failed
startup burst but kept the adjacent `Status` value green as `healthy`. That status considered
only the terminal error, which is populated after the workload joins all task handles, not a
failure already observed while the test is running.

Decision: make any nonzero task-failure count immediately render a red `degraded` status with
the count. A terminal error still takes precedence and is shown verbatim.

Tradeoffs: the dashboard cannot yet assign an individual failure reason to each room tile, but
it no longer suggests that an actively degraded workload is healthy.

## 2026-08-20: Runtime stress separates setup and live-audio deadlines

Context: a 100-session loopback run reported `deadline has elapsed` while only part of the
workload had started. The error originated in the harness's two-second WebSocket receive guard,
which it used both for a session-start control message and for a live PCM frame. Session start
can legitimately contend on SQLite, WebSocket upgrades, and agent/ASR admission under a ramp;
two seconds measures the test harness's impatience rather than an audio or protocol failure.

Decision: allow thirty seconds for game-control startup messages, while retaining the two-second
deadline for verified live PCM relay frames. Startup latency remains recorded in the dashboard and
JSON report, so a slow ramp stays observable without invalidating an otherwise healthy workload.

Tradeoffs: a genuinely stalled session now takes longer to fail during setup. This is intentional:
the run has a visible finite admission deadline while preserving a much stricter requirement for
ongoing audio delivery.

## 2026-08-20: Runtime stress fixture scales ASR admission with synthetic sessions

Context: the runtime stress harness uses a local transcription provider so that each
browser-like human exercises the real ASR admission and audio-ingestion paths. It raised
the active-room, waiting-room, and unattached-participant limits but inadvertently retained
the ordinary deployment default of 32 transcription streams. A 100-session human-agent run
therefore stopped admitting rooms at 32 with an HTTP 503 before it could measure the requested
workload.

Decision: set the fixture's transcription-stream reservation to the same stress-only scale as
the other synthetic capacity reservations (4,000). The production configuration defaults and
their admission controls remain unchanged.

Tradeoffs: this removes a useful deployment-limit test from the default workload command.
Capacity-policy verification should instead run explicitly with configured production limits;
the runtime stress tool's primary purpose is to measure protocol, SQLite, and provider-path
behavior at the chosen scale.

## 2026-08-20: Immutable experiment identity is not repeated in Configuration

Context: the Configuration panel displayed an experiment's immutable identifier in a disabled
text input even though the same identifier is already the selected experiment's panel title.
Although technically non-editable, the input styling implied that the value belonged to the
editable configuration form.

Decision: omit the experiment ID from Configuration and use the selected experiment title as its
single display location. Retain the editable ID input in the creation dialog, where the researcher
must choose the identifier before it becomes immutable.

Tradeoffs: the identifier is not repeated beside the configuration values, reducing redundancy at
the cost of requiring users to refer to the persistent panel title when confirming the experiment.

## 2026-08-20: Desktop dashboard regions own scrolling below fixed navigation

Context: allowing the embedded dashboard document to grow made the product header and selected
experiment controls scroll away, while constraining the workspace without assigning overflow to
tab panels made Configuration and Privacy content unreachable.

Decision: bound the desktop document to the viewport and keep overflow inside the experiment and
session catalogues, event log, experiment tab panel, and game-level page panel. The product header,
selected experiment header, and experiment tabs remain outside those scroll containers. At the
narrow-screen breakpoint, restore document scrolling for the stacked mobile layout. Experiment
tab scroll containers span the full workspace so their scrollbar stays at the right edge. Their
content uses fluid viewport-relative gutters instead of a fixed maximum width, allowing wide
configuration controls to use the available workspace while retaining padding at every width.

Tradeoffs: nested scroll regions require explicit `min-height: 0` throughout the flex/grid chain
and careful mobile overrides. They preserve stable navigation and independent long lists on desktop
without imposing desktop overflow behavior on small screens.

## 2026-08-20: Dashboard polling never reconstructs an active source editor

Context: the experiment catalogue refreshes periodically. Re-rendering the Notes editor during
that refresh replaced its textarea node, which blurred the field and interrupted typing even
though neither the selection nor the note had changed.

Decision: keep the source editor DOM stable for the lifetime of an experiment selection. Polling
may refresh non-dirty source in place but never replaces the textarea, resets its selection, or
changes focus. Only an explicit experiment selection constructs a different editor. Markdown
highlighting is rendered in a passive layer behind the textarea, and Write/Preview use the same
underline-tab visual language as the dashboard workspace.

Tradeoffs: the editor owns a small amount of synchronized DOM state for highlighting, line
numbers, and scrolling. This avoids a larger client framework dependency while keeping polling
independent of keyboard interaction.

## 2026-08-20: Catalogue badges separate configuration validity from runtime readiness

Context: a stored experiment can be structurally valid yet unable to start because a required
provider credential or other activation prerequisite is missing. Treating both cases as a generic
configuration error hid that distinction and made archived-but-valid experiments appear broken.

Decision: assess every catalogue entry against the compiled game schema and expose separate
validity and runnable facts with hover diagnostics. The dashboard shows compact badges for both,
uses the exact `GameMetadata.name`, and offers Unarchive only when the valid runtime route can be
constructed. Structurally invalid archived entries retain only their storage-only archive state.

Tradeoffs: loading the catalogue performs bounded validation work for each experiment. This keeps
readiness truthful without constructing or caching experiment routers, at the cost of additional
configuration and credential reads during administrator catalogue refreshes.

## 2026-08-20: Space Game names the local Parlando development run explicitly

Context: Space Game already uses sibling `rust-server` and `js-client` path dependencies, but
its development command was named only `run`, making the local dependency contract easy to miss.

Decision: add `make run-local` as the explicit target that prepares both sibling artifacts and
starts the game. Keep `make run` on the published manifest dependencies; `run-local` uses a
Cargo command-line patch plus a no-save npm install instead of changing either manifest.

Tradeoffs: local preparation takes an additional override step, but the explicit name makes
testing an unpublished Parlando checkout discoverable without changing published-release tests.

## 2026-08-20: Invalid experiment configurations have a one-way catalogue archive path

Context: normal experiment-scoped administration constructs and validates the stored runtime
configuration. That prevented an administrator from archiving an old configuration after a game
schema change, even though archival changes only durable catalogue lifecycle state.

Decision: add a dedicated installation-routed archive endpoint that atomically permits only
`inactive | completed → archived`, then evicts any cached router. It never invokes experiment
runtime construction. All other operations, including restore, remain runtime-scoped and keep
their configuration validation requirement.

Tradeoffs: an invalid archived experiment cannot be restored through the dashboard. This is a
deliberate one-way safety boundary that avoids turning legacy archival support into a general
configuration-validation bypass.

## 2026-08-17: Render deployment is documented through one runnable example

Context: The general deployment reference described Render in a short operational
section, and Space Game already contained a Dockerfile and minimal Blueprint.
However, a new game author still had to infer the repository-root Docker context,
the relationship between the persistent-disk mount and SQLite URL, and the
first-deploy workflow.

Decision: Add a task-oriented Render tutorial that starts with the complete Space
Game example and then explains how to adapt it. Keep the Dockerfile and Blueprint
as the executable source of truth. Make the Blueprint explicit about its
repository-root build context and non-secret Parlando environment variables, and
link the tutorial from the Space Game README, documentation index, and general
deployment reference.

Tradeoffs: The focused tutorial repeats a small part of the broader deployment
reference, but it gives readers one linear path they can execute without mixing
platform setup with all operational topics. Provider keys remain absent from the
example because typed deployments do not need them and committed placeholders
can encourage unsafe secret handling.

Follow-up risks: Render can revise its Blueprint schema, plans, and persistent
disk behavior. Revalidate `space-game/render.yaml` against Render's schema when
those platform contracts change, and keep the tutorial synchronized whenever the
Space Game Dockerfile, binary name, or bootstrap environment changes.

## 2026-08-15: Reliability tests are invariant-driven and deterministic

Context: The Rust server already has broad happy-path integration coverage, while the JavaScript client primarily tests pure helpers. The remaining reliability risk is concentrated in durable-write failures, asynchronous replacement and cleanup races, provider backpressure, browser lifecycle interleavings, and cross-language protocol drift. A line-coverage target alone would not prove these behaviors and timing-based sleeps would make the most important tests flaky.

Decision: Organize the comprehensive suite around explicit authorization, durability, exactly-once transition, isolation, bounded-queue, generation-ownership, timer, and wire-contract invariants. Use paused clocks, barriers, controlled futures, local fake providers, and a fault-injecting storage implementation to force each important interleaving. Use line and branch coverage as a ratcheted omission detector, with stricter branch coverage on authentication, lifecycle, redaction, framing, and client generation guards. Keep paid providers out of automated tests; exercise production adapters against local protocol peers and reserve real browser/network conditions for browser and scheduled soak lanes. The complete catalogue and acceptance rules live in `notes/comprehensive-test-design.md`.

Tradeoffs: The harness requires additional test-only dependencies and explicit injection points, and deterministic race tests take more design work than broad end-to-end scripts. In return, failures identify a violated property and remain reproducible. Browser and soak lanes take longer than the ordinary unit suite, so CI is split into fast, integration, browser, and scheduled lanes instead of running every workload on every edit.

Follow-up risks: Coverage thresholds must be introduced as a ratchet from a measured baseline rather than as an arbitrary one-time gate. Mutation and fuzz results are diagnostic and require triage; they must not become noisy release metrics. If a production path cannot be controlled deterministically in a test, extract the clock, id source, queue, or persistence boundary rather than adding longer sleeps.

## 2026-08-15: Session polling refreshes durable metadata and runtime health together

Context: the dashboard polls durable session summaries and game-wide runtime health every five seconds. It rerendered the session list from the durable response but retained a separate selected-session object loaded only when the row was clicked. After an intentional departure, health could therefore become `Unavailable` while the selected header continued showing the stale lifecycle `Running` until a full page reload.

Decision: whenever the durable session catalogue refreshes, merge the matching summary into the selected-session object and rerender its metadata header. Keep runtime health independently sourced from the load endpoint, but present both values from the same polling cycle. If a status filter removes the transitioned session, clear the selection as before rather than displaying an item outside the chosen filter.

Tradeoff: the compact summary refreshes lifecycle, timestamps, and counts without refetching the full participant and event payload every five seconds. Participant rows remain refreshed on explicit selection, while the existing 1.5-second event poll handles new log entries.

## 2026-08-15: Session start is a durable `running` transition

Context: rooms were created durably as `waiting`, while `maybe_start_game` changed only the process-local room to `playing`. The dashboard reads SQLite, so a session could be visibly running for participants yet remain `waiting` until a terminal completion, abandonment, or expiry overwrote it.

Decision: use the lifecycle `waiting → running → completed | abandoned | expired`. Once both roles and any required speech-recognition setup are ready, atomically update the SQLite session to `running` and set `started_at` before changing process-local state or announcing role assignment. If that durable transition fails, do not start game execution. Replace `playing` with `running` in runtime logic, telemetry, filters, tests, and existing stored rows because “running” applies equally to human, agent, and mixed sessions.

Tradeoff: database write availability is now part of the start gate, which can delay or prevent a room from starting during storage failure. This preserves the stronger invariant that participant-visible execution never advances beyond the durable lifecycle record.

## 2026-08-15: Intentional departure abandons the session

Context: the reusable client implemented “Leave game” by closing its WebSocket. The server correctly treats socket loss as recoverable so participants can reconnect after transient network failures, but this left intentionally ended rooms durably marked `waiting` or `playing` until a timeout expired them.

Decision: send an explicit `leave` game-channel message before the participant client closes its transports. Atomically record `session_abandoned` and change the durable session status to `abandoned`, then notify any other connected participant that the room ended. Browser tab closure, transport loss, and crashes continue to produce only `participant_disconnected` and retain the configured reconnect grace period. Abandonment is terminal and distinct from successful `completed` sessions and automatic `expired` sessions.

Tradeoff: delivery depends on the WebSocket being open when the participant presses the button. If it is already disconnected, the server cannot distinguish that click from the preceding network failure and the normal reconnect timeout applies. This preserves truthful failure semantics without introducing a second authenticated HTTP termination path.

## 2026-08-15: Configuration editability follows authoritative lifecycle data

Context: the dashboard keeps a client-side experiment catalogue. After a game-server restart resets open intake to inactive, an already-open dashboard can retain the pre-restart `active` or `testing` value. The configuration endpoint returns the current experiment definition, but the editor previously ignored its lifecycle fields and disabled controls from the stale catalogue object.

Decision: whenever configuration loads, reconcile the returned authoritative experiment definition into both the selected experiment and catalogue entry, rerender lifecycle-dependent UI, and compute editor read-only state directly from that response. Also reapply editor availability after every catalogue refresh, even when the configuration revision is unchanged; lifecycle-only transitions such as ending testing must update the existing controls rather than leave their prior `disabled` attributes in place. Configuration remains editable only for an inactive experiment on the current compiled game version.

Tradeoff: the normalized configuration response now also refreshes catalogue metadata opportunistically. This avoids an additional catalogue request and keeps the form correct across restarts, while the next ordinary catalogue refresh remains authoritative for session aggregates and other experiments.

## 2026-08-15: Process startup always closes experiment intake

Context: retaining `active` or `testing` across a game-server restart can reopen participant intake after a crash, deployment, configuration change, or unattended machine restart without an experimenter reviewing readiness.

Decision: after opening storage and ensuring the bootstrap experiment exists, atomically change every experiment in `active` or `testing` to `inactive` before constructing any runtime. Preserve `completed` and `archived`, because they are terminal catalogue/data states rather than intake states. Starting testing or research intake is always an explicit authenticated dashboard action after the current process is running.

Tradeoff: routine deployments and crash recovery require one deliberate activation step per experiment. This favors fail-closed recruitment and operator awareness over automatic availability.

## 2026-08-15: Experiment dispatch preserves transport upgrade state

Context: the multi-experiment host rewrites `/e/{experiment_id}/...` requests into an isolated child router. Axum stores outer wildcard path captures in request extensions, so the dispatcher cleared those extensions before entering the child routing boundary. That also removed Hyper's `OnUpgrade` handle. HTTP APIs and WebSocket-plan generation therefore succeeded, but game and audio WebSocket upgrades never reached their handlers; a human-versus-agent room remained waiting with only the in-process agent shown as connected.

Decision: preserve and restore both the peer `ConnectInfo` and Hyper `OnUpgrade` extension while clearing outer routing captures. Exercise a real WebSocket handshake through the compiled-game experiment dispatcher and assert that the human presence becomes connected. In the participant gate, distinguish a transcription service that has not started because the game transport is disconnected from one that is actively progressing through provider startup; surface game-channel connection failures while automatic reconnection continues.

Tradeoff: the dispatcher explicitly knows about the transport-level extensions required across its synthetic routing boundary. Any future extractor that depends on another server-injected request extension must be evaluated here and covered by an end-to-end routed test.

## 2026-08-15: Participant intake follows the experiment lifecycle

Context: the server admits participants while an experiment is either `testing` or `active`, but the reusable participant client originally enabled its waiting-room action only for `active`. This made the UI appear complete in testing mode while silently disabling entry.

Decision: the shared client models all five server lifecycle values and centralizes intake eligibility as `testing | active`. Closed states (`inactive`, `completed`, and `archived`) use the same closed-intake presentation and polling behavior. Testing sessions retain the server-assigned `testing` data purpose and remain excluded from research exports.

Tradeoff: the participant UI deliberately does not advertise whether a closed experiment is inactive, completed, or archived. Those distinctions matter to experimenters, while participants only need to know whether intake is open.

## 2026-08-15: Public-intake controls remain analyzable and dashboard-owned

Context: The final launch review identified several realistic but bounded failure modes for self-hosted academic crowdsourcing: anonymous self-pairing, rejected-action disk growth, CPU-heavy administrator login, misleading full-state privacy behavior, abandoned voice/agent capacity, unreproducible clean builds, and health or backup blind spots. Treating the whole set as a categorical launch "no-go" overstated some limitations and obscured the product choices already accepted in the authoritative threat model. An initial implementation also placed administrator CIDR policy in an environment variable, contrary to Parlando's database-backed configuration model.

Decision: Describe self-pairing as an accepted study-validity limitation that remains available for local testing; do not add identity tracking or attempt to prohibit it in the core. Limit action submissions to 20 per participant and 300 process-wide per minute, cap serialized actions at 4096 bytes, and persist bounded `game_action_rejected` records with stable reason codes, byte counts, and SHA-256 fingerprints instead of raw rejected input. Coalesce repeated rate-limit records to one per participant per minute. Store administrator CIDR ranges in shared SQLite game settings and edit them in the dashboard; the application checks only the direct transport peer and does not trust forwarded IP headers. Run at most two Argon2 verifications concurrently on blocking workers and reduce total HTTP request concurrency to 256. Honor `privacy.store_full_game_state=false` for both the state column and embedded pre-action state. Expire waiting, disconnected, idle, and over-lifetime sessions with durable terminal reasons. Require locked Rust and npm inputs for container builds, return `Cache-Control: no-store` on administrator and API responses, make health depend on an SQLite write transaction plus read, and document off-service restore-tested backups. First-visitor administrator claiming remains explicitly out of scope.

Tradeoffs: A person can still operate both anonymous roles, and rate limits constrain accidental high-frequency games as well as attackers. Rejection fingerprints support grouping without retaining the rejected content itself. Direct-peer CIDRs work on direct deployments but see the proxy address when an ingress hides clients; those installations must enforce the same policy at the trusted proxy. An administrator can save a range that excludes their current network, so deployment documentation must make the lockout behavior clear. Session expiry bounds resource occupation but does not stop a person from creating another anonymous session.

Follow-up risks: Monitor rejection counts, database growth, expired-session reasons, provider concurrency, and disk usage during intake. Revisit the numerical limits if a real game needs faster legitimate actions. Test administrator CIDRs in the actual network topology before recruitment, test a backup restore rather than only backup creation, and keep the tracked lockfiles synchronized with dependency changes. Study and operational policy remains dashboard-owned; environment values are reserved for non-persisted secrets and coordinates required before the database/dashboard can open.

## 2026-08-15: Security reviews follow an explicit academic-research threat model

Context: A generic security review can misclassify intentional product choices as vulnerabilities and direct effort away from attacks that matter for a small, self-hosted crowdsourcing deployment. In particular, Parlando intentionally lets the first visitor create the administrator for a fresh database, and anonymous intake cannot recognize a returning natural person without collecting an identity or linkability signal. The older remediation record captures useful implementation history but is not a durable definition of accepted risk.

Decision: Make `docs/security-ground-rules.md` the authoritative threat model and finding policy, superseding older audits, plans, and checklists wherever they conflict. Treat first-visitor administrator creation as an accepted bootstrap ceremony that future reviews must not report as a security issue; continue to require atomic singleton creation and authentication after setup. Treat person-level repeat participation as a recruitment-design and data-quality matter, not a Parlando vulnerability, and prohibit browser fingerprinting, IP uniqueness, durable cross-study identifiers, or comparable covert tracking as a purported fix. If a study needs one use per issued invitation, the preferred optional control is an atomically redeemed high-entropy admission capability of which Parlando stores only a hash. Blind-signed capabilities may be supplied by an external issuer when unlinkability between issuance and redemption is itself required.

Tradeoffs: The policy accepts that an operator must claim a new installation before announcing it and that anonymous Parlando intake cannot prove one human contributed only once. It distinguishes duplicate participation from automated resource, cost, and large-scale dataset abuse, which remain security concerns. Hashed single-use capabilities prevent invitation replay without adding identity data, but cannot prevent one person from obtaining multiple legitimate invitations or accounts; recruitment systems remain responsible for that boundary.

Follow-up risks: Keep the ground rules aligned with actual deployment promises. If Parlando later advertises person-level uniqueness, implements admission capabilities, accepts mutually distrustful tenants, or begins storing high-impact regulated data, revise the threat model deliberately and audit the new boundary. Existing references to the remediation record are historical only and must not restore it as the source of current policy.

## 2026-08-13: Administrator setup is browser-first and database-backed

Context: Requiring operators to install a separate Argon2 command-line tool, generate a PHC hash, and export multiple environment variables made ordinary local administration needlessly difficult. Parlando already owns a persistent database and a protected login surface. The intended deployment workflow assumes that the operator is the first visitor to a newly created database.

Decision: When no administrator credential exists, `/admin` redirects to `/admin/login`, which presents a first-run account-creation form. The server validates the username and a password of at least 12 characters, hashes the password with Argon2id, and atomically inserts one singleton credential row containing only the username, PHC hash, administrator role, and creation time. The successful setup request receives a normal authenticated session. Subsequent setup requests fail, and future visits receive the login form. Setup and login use the dashboard's server-rendered visual language and remain usable without the game client bundle. This first-visitor setup is available wherever the server is bound, by explicit product choice. A later configuration-authority decision removed the then-available environment credential override; administrator identity is now database-backed only.

Tradeoffs: First-run operation is simple and credentials persist across server restarts and binary reinstalls. A newly exposed database has a deliberate claim window until its operator completes setup, so deployment instructions require doing so before distributing the URL. Each SQLite database has an independent administrator, which means the Space Game's normal and solo-voice local databases each require setup once. Reinstalling the executable does not reset a database-backed credential.

Follow-up risks: A deployment model in which URLs become publicly reachable before operators can visit them should add an explicit provisioning or invitation mechanism. Password reset and administrator rotation are not yet exposed in the UI; current recovery requires a stopped-service database repair or restore.

## 2026-08-13: The monorepo Space Game exercises the local Parlando libraries

Context: The Space Game README and Dockerfile described and copied the repository-local Rust server and JavaScript SDK, but `space-game/server/Cargo.toml` still resolved `parlando-server 0.2.0` from crates.io. Local and image builds could therefore combine the hardened browser SDK with the older published Rust protocol implementation, making security and voice regressions difficult to interpret and leaving the copied Rust source unused.

Decision: Make both Space Game manifests explicit development consumers. The Rust dependency uses the repository-relative `../../rust-server` path without a registry version, and the npm dependency uses `file:../../js-client`. The monorepo demo, its local Make targets, tests, and Docker build now compile the exact Rust and browser SDK sources beside it. Independent generated games remain registry-package consumers under the game-generation contract.

Tradeoffs: The demo crate is intentionally tied to the monorepo layout and emits its existing local-dependency reproducibility warning. This is appropriate for an integration fixture and public-release example because it prevents a locally tested source tree from silently shipping an older server. A separately distributed Space Game source archive would need either both directories or a release manifest rewritten to the coordinated published versions.

Follow-up risks: Publish the Rust server and JavaScript SDK as a coordinated compatible release before treating an external generated game as evidence for this source tree. Release packaging should verify that Docker and local binaries report the expected server/client revisions and that voice-enabled configuration exposes microphone preparation while voice-disabled configuration omits it.

## 2026-08-13: SQLx is limited to Parlando's SQLite runtime surface

Context: `cargo audit` reported the patched-in-place `event-listener` soundness advisory and the unfixed `rsa` timing advisory. SQLx Core introduced `event-listener`; SQLx's default macro and multi-driver resolution retained the MySQL/RSA branch in `Cargo.lock`, even though Parlando opens only SQLite databases and `cargo tree` showed that RSA was not in the compiled dependency graph.

Decision: Keep SQLx on the stable 0.8.6 line, disable its default features, and enable only `runtime-tokio`, `sqlite`, `chrono`, and `json`. Parlando uses runtime `query`, `query_as`, and `query_scalar` calls and does not use SQLx query macros, migrations, `Any`, MySQL, or PostgreSQL. Refresh lockfiles to select `event-listener` 5.4.2 or newer, which contains the upstream soundness fix. Upgrade Tonic and Tonic Build from 0.12 to the compatible 0.13.1 release and select its explicit native-root and Ring TLS features, removing Tonic's dependency on the unmaintained `rustls-pemfile` wrapper. Defer the breaking SQLx 0.9 and Tonic 0.14 migrations because neither is required to resolve the audited dependency paths.

Tradeoffs: The compiled dependency and feature surface is smaller. Adding compile-time SQL macros, another database backend, or SQLx migrations later will require an explicit feature change and review. Cargo still records SQLx's optional MySQL/RSA packages in the lockfile even though `cargo tree` proves they are absent from the compiled graph, so RustSec requires a documented reachability exception until SQLx's package structure or the RSA implementation changes. Staying on SQLx 0.8.6 and Tonic 0.13.1 avoids mixing larger API and protobuf-codegen migrations into the security hardening work.

Follow-up risks: Every deployable consumer has its own Cargo lockfile. Until the hardened `parlando-server` release is published and consumed, Space Game builds must resolve the local hardened crate when refreshing or auditing their lockfile; auditing the previously published 0.2.0 dependency will continue to describe that older package's dependency graph.

## 2026-08-13: Security remediation separates admin, participant, and secret trust planes

Context: The public-release security audit found that administrative routes were unauthenticated, serializable experiment configuration contained provider credentials, and participant session identifiers were both disclosed and accepted as bearer credentials. Fixing those problems independently would leave alternate impersonation and data-exfiltration paths through HTTP, WebSocket, export, presence, and provider integrations.

Decision: Implement the high-severity remediation as three explicit trust planes. Administrative routes use application-owned authenticated sessions, roles, and CSRF protection. Participants receive a non-secret public identifier plus a separate opaque credential; authenticated HTTP calls resolve the participant principal from that credential, while game and audio WebSockets use short-lived one-use purpose-bound tickets. Runtime provider, administrator, ticket-signing, and remote-agent secrets are loaded into non-serializable secret types and are never included in persistable experiment configuration or export. The phased design and release gates from that remediation effort were recorded in the now-historical `notes/security-remediation-plan.md`; it is not the current threat-model authority.

Tradeoffs: This is a coordinated breaking change across the Rust server, JavaScript SDK, Python agent SDK, storage schema, deployment configuration, and generated-game guidance. It adds session and ticket lifecycle state, migration tooling, and stronger production configuration requirements. In return, identifiers can safely appear in research data without authenticating requests, and no deployment must rely on a reverse proxy as its only application security boundary.

Follow-up risks: Authentication recovery, OIDC integration, proxy-aware client-IP handling, conditional secret rotation, and multi-instance ticket/session storage need careful interfaces. Compatibility modes must not accept old participant IDs as credentials or expose old admin routes, because doing so would preserve the audited vulnerabilities. Testing to date was internal and provides no evidence that Speechmatics or ElevenLabs credentials were compromised. Affected databases and build caches require explicit redaction and exposure review; rotate only credentials found outside the controlled boundary, retained in shared artifacts, or covered by an applicable rotation policy before public intake begins.

## 2026-08-13: Public-release security uses process-local opaque credentials and fail-closed transitions

Context: Parlando currently owns active rooms in one Rust process, while durable evaluation data is stored through the experiment-store abstraction. Closing the audited authorization and integrity paths before public release required a design that works for the current single-process deployment without pretending public IDs, reverse-proxy controls, or best-effort logging are authentication or durability boundaries.

Decision: Keep participant and administrator credentials, credential generations, one-use WebSocket tickets, CSRF values, and connection-cancellation handles in bounded process-local registries. Store only participant credential hashes and secret-free experiment configuration. Authenticate participant HTTP requests with bearer credentials and browser upgrades with short-lived purpose-bound tickets. Authenticate administrators with Argon2id and server-side secure-cookie sessions. Serialize each room's action transitions and atomically commit accepted/state/completion rows before changing memory or broadcasting. Require HTTPS, runtime bearer authentication, and an exact host allowlist for non-loopback remote agents; retain literal-loopback cleartext only for local development.

Tradeoffs: Restarting the process invalidates active credentials and tickets, and horizontal deployments still require sticky routing or a shared authentication/room backplane. The coordinated browser SDK change is intentionally incompatible with ID-bearing game URLs. Keeping provider values in the parsed configuration long enough to construct providers avoids a larger configuration-loader break, while extracting them before persistence and applying recursive storage/export redaction closes the current exposure path.

Follow-up risks: A future multi-instance architecture should move session and ticket state into a shared, atomic store and add explicit participant recovery/rotation. Per-client-IP policy needs trusted-proxy configuration before forwarded addresses can be used safely; current process ceilings remain the safe fallback. Public release still requires migration rehearsal, hostile load tests, container/SBOM scans, deployment smoke tests, and independent security review. Credential rotation remains conditional on the internal artifact review or applicable policy, not presumed compromise.

## 2026-08-13: Release tooling pins patched transitive build dependencies

Context: The 0.2.0 package dry-run warned that the Rust lockfile selected yanked `spin` 0.9.8 through SQLx and npm audit found vulnerable PostCSS and Nanoid patch releases through Vite and Vitest. These dependencies support storage or local build/test tooling; Parlando does not call their affected APIs directly, but release preparation should still resolve known warnings reproducibly.

Decision: Update Spin within its existing 0.9-compatible series to non-yanked 0.9.9. Add npm overrides for patched PostCSS 8.5.26 and Nanoid 3.3.18 in both JavaScript workspaces, retaining the current Vite and Vitest major versions while ensuring fresh installations cannot restore the vulnerable transitive patches.

Tradeoffs: Overrides make the security floor explicit and avoid unnecessary framework-major upgrades, but they require periodic review because npm's normal transitive resolution is intentionally constrained. The JavaScript packages use these dependencies only for development and packaging; the published SDK remains dependency-free apart from its React peer requirement.

Follow-up risks: Remove or advance the overrides once the direct Vite and Vitest dependency ranges consistently resolve patched versions without them. Continue treating audit output as release input rather than applying unreviewed `npm audit fix --force` upgrades.

## 2026-08-13: Release 0.2.0 marks the server-owned audio transport boundary

Context: Replacing the previous browser media dependency with Parlando's authenticated PCM relay changes the voice transport, provider boundary, browser startup behavior, and deployment assumptions. The Rust server crate and JavaScript client must move together because the transport protocol and startup gate are coordinated across both packages.

Decision: Release `parlando-server` and `@coli-saar/parlando-client` together as 0.2.0, update consumer examples and the game-generation fallback to that aligned version, and keep the Space Game demo pointed at the matching registry version while its local development workflow continues to install the checkout explicitly.

Tradeoffs: A minor-version bump in the pre-1.0 series communicates that consumers must upgrade the server and browser SDK together. It is more disruptive than a patch release, but avoids presenting the audio transport replacement as backward-compatible.

Follow-up risks: Downstream games must update both package constraints and lockfiles. Deployments must also adopt the documented WebSocket routing, sticky-session, Speechmatics server configuration, and browser worklet requirements before moving to 0.2.0.

## 2026-07-13: Release 0.1.3 keeps package versions aligned

Context: The reusable Rust server crate and JavaScript client package are released together, and the game-generation skill carries an offline fallback version for generated manifests. Preparing 0.1.3 needs the package metadata, lockfiles, changelog, and release guidance to tell the same story.

Decision: Bumped `parlando-server` and `@coli-saar/parlando-client` from 0.1.2 to 0.1.3, updated lockfile package entries that point at the local reusable server crate, and added a 0.1.3 changelog section focused on user-visible runtime, documentation, and generation changes.

Tradeoffs: The Python agent SDK remains on its independent 0.1.0 version because the existing changelog and publishing docs define the synchronized release train as the Rust server crate and JavaScript client package. Demo app package versions also remain independent from reusable package releases.

Follow-up risks: If the Python SDK becomes a published artifact in the same release train, future release prep should document that and bump its `pyproject.toml` alongside the Rust and JavaScript packages.

## 2026-07-13: Browser teardown uses the Leave Game cleanup path

Context: Games such as Space Game expose a `Leave game` button by calling `session.leave()` from the reusable React startup gate. Closing the browser tab or window should be interpreted the same way, so the server records the participant as disconnected and other participants see presence update.

Decision: The startup gate now keeps the active room session in a ref and calls the same socket/audio cleanup used by `session.leave()` from `pagehide`, `beforeunload`, and React unmount. The cleanup explicitly closes the game WebSocket when it is open or connecting; the Rust server already treats WebSocket closure as `participant_disconnected`.

Tradeoffs: Browser unload events do not allow reliable asynchronous work, so this avoids a new HTTP leave endpoint and relies on WebSocket close semantics. Audio disconnect is still requested, but browser shutdown may terminate media cleanup early.

Follow-up risks: If future study logic needs a durable distinction between intentional leave, browser close, network loss, and crash, add an explicit leave reason protocol instead of overloading `participant_disconnected`.

## 2026-07-13: README records native build toolchain floors

Context: A contributor hit a `make install-server` failure while compiling a Parlando game server on macOS, and the fix was updating Apple Command Line Tools for Xcode to 16.0.0. The README previously described how to run the server but did not state the native compiler, SDK, Node, or Make requirements up front.

Decision: Added a root README system requirements section that documents current stable Rust, the Node versions required by the Vite 7 browser build, GNU Make, the standard macOS Command Line Tools required by Rust, and the Linux native-toolchain expectation.

Tradeoffs: The Rust requirement remains "current stable" rather than a pinned MSRV because the repository does not yet carry a `rust-toolchain` or `rust-version` contract. Platform toolchain guidance is intentionally limited to what the current Rust, SQLite, protobuf, and browser builds require.

Follow-up risks: If the project later pins Rust or changes Vite major versions, update the requirements section at the same time.

## 2026-07-13: Startup gate does not expose manual room selection

Context: A proposed fix for duplicate Player A reports added a generic Room ID field to the reusable React startup gate. That made room pairing a participant-facing manual step in every generated client, even though room routing is study-specific and should not leak into the default SDK startup UI.

Decision: Keep `ParlandoStartupGate` focused on participant setup, consent, voice preflight, and waiting-room readiness. Do not add a generic Room ID selector, Room ID prop, or URL-query room selection behavior to the reusable startup gate. Human-human entry is paired on the server: `POST /api/rooms` fills an existing compatible waiting room before creating a new Player A room. The public API has no caller-selected room join operation.

Tradeoffs: Ad hoc manual joining is intentionally unsupported, but generated clients avoid exposing an implementation detail to participants. Server-side first-open-room pairing is intentionally simple; future studies that need cohorts, treatments, counterbalancing, or private invite links should add an explicit server-side pairing policy.

Follow-up risks: The duplicate Player A class of bug should stay covered by tests that exercise two independent default waiting-room entries. If pairing policy grows more complex, preserve that default behavior or make the replacement policy explicit in configuration.

## 2026-07-13: Documentation starts from researcher workflows

Context: The README and user-facing docs were technically accurate but led with runtime infrastructure before making the research workflow explicit. New researchers need to see quickly whether Parlando fits their study, while the docs still need precise implementation contracts.

Decision: Reframed the README around study fit, examples, and provided capabilities before adapter details. Updated the docs index and technical pages to introduce workflows, current route names, analysis-oriented completion summaries, and deployment boundaries without changing implementation contracts.

Tradeoffs: The overview now repeats a small amount of information from the technical pages, but that duplication helps first-time readers decide where to go next. The detailed docs remain the source of truth for protocol and deployment specifics.

Follow-up risks: The generation skill is now available and the README reflects it. If recruitment-provider automation is added later, document the verified server-controlled identity flow rather than implying that the public participant endpoint accepts provider identities.

## 2026-07-13: Agent action responses apply before speech

Context: An agent response can contain both an action and a message. The runtime previously persisted and spoke the message before submitting the action. With TTS enabled, that meant the visible game action could be delayed until after speech synthesis and playback, making the UI feel out of order.

Decision: Combined agent responses now apply and broadcast the action first, then persist and speak the message. If the action is rejected, the paired message is not emitted. Message-only responses still work without a state broadcast.

Tradeoffs: This makes action/message pairs behave as “do this, then say the explanation” rather than “say this while waiting to do it.” Agents that want to speak before acting should return a message-only response and wait for a later observation before returning the action.

Follow-up risks: If future games need simultaneous animation and speech timing, the response contract may need explicit timing metadata. For now the deterministic action-first order matches the current UI and voice expectations.

## 2026-07-13: Agent decisions wait for the next observation

Context: The event-driven agent runtime called `maybe_act` at the top of its loop. After an agent response was applied, the loop could ask for another decision before delivering the queued observation for the agent's own accepted action or message. In the Space Game demo this was visible as the built-in agent taking a quick second step, especially when TTS delayed action submission long enough for the demo agent's one-second throttle to expire.

Decision: The agent loop now performs the initial `observe_state` followed by one decision, then waits for an observation before asking for the next decision. Agents still receive observations for accepted actions and messages, including their own effects, but each follow-up decision is causally tied to an observation that has already been delivered. The Space Game back-and-forth demo agent also tracks whether a step is pending, so it takes one initial step and then one step per other-player movement.

Tradeoffs: Agents that intentionally want autonomous continuous behavior should schedule that through explicit observations or a future timer mechanism rather than relying on the runtime spinning `maybe_act`. This keeps the base contract closer to the event-driven model and avoids accidental self-triggered action chains.

Follow-up risks: A future runtime may need first-class timer observations for agents that should act without human or game events. That should be added explicitly rather than recreating an implicit polling loop.

## 2026-07-13: Changelog uses release-focused entries

Context: The repository did not have a durable changelog, and the current worktree already contains unreleased agent API changes intended for 0.1.2. The 0.1.0 and 0.1.1 releases can be reconstructed from package versions and the release commit, but they do not have annotated git tags with release dates.

Decision: Added a top-level `CHANGELOG.md` using a Keep-a-Changelog-style structure, with `0.1.2` marked as `Unreleased`, `0.1.1` reconstructed from the version bump commit, and `0.1.0` described as the first packaged baseline.

Tradeoffs: The reconstructed entries summarize major user-visible and contributor-visible changes rather than attempting an exhaustive commit log. Dates are omitted until the project has explicit release date metadata.

Follow-up risks: Future release work should fill in dates when versions are cut and keep the unreleased section current as implementation work lands.

## 2026-07-13: Agents observe events before deciding

Context: The original agent API called `act(observation, available_actions)` on a timer. That let agents see current state, but it did not give them a clean way to observe the other participant's utterances or ingest multiple state/message changes before responding.

Decision: Replaced the polling contract with event-driven callbacks: `observe_state`, `observe_action`, and `observe_message`, followed by `maybe_act` or `act`. Agent responses are represented as `AgentResponse { message: Option<String>, action: Option<Action> }`, with empty responses rejected. `available_actions` is passed only to decision methods. The remote gRPC protocol was bumped to `parlando-agent-v2` and mirrors the same observation/decision split.

Tradeoffs: This is a clean breaking change for Rust and Python agents, but it removes the overloaded `AgentResult::None` result and makes speech/actions independent capabilities. Agents that need history now store it explicitly in their per-session instance.

Follow-up risks: Existing external agents must be migrated to v2 before use. Future forced-turn or RL runtimes can call `act` when they require a non-empty response, and can layer stricter "must include action" rules outside the base trait.

## 2026-07-12: Generated games stay participant- and voice-agnostic

Context: The Parlando game-generation skill still described voice and agent modes in ways that could lead generated games to decide whether voice is enabled or to branch browser/game behavior around human-vs-human versus human-vs-agent play.

Decision: Clarified the skill and references so generated games treat participant composition and voice enablement as Parlando runtime concerns. A browser game instance renders one human player's role-specific UI, sends that player's actions, and receives the other participant's accepted actions/events through the SDK. The game must allow SDK-provided voice behavior when the server/session exposes it, and omit or disable voice UI from session state when it does not.

Tradeoffs: This keeps game code simpler and more reusable across human/human, human/agent, and voice/non-voice deployments. It also means generated games rely on the server and client SDK to expose accurate capability metadata and session controls.

Follow-up risks: Future skill updates should preserve this boundary when adding new communication modes or participant types.

## 2026-07-12: Generated clients style SDK startup screens

Context: `@coli-saar/parlando-client/react` now provides `ParlandoStartupGate`, which centralizes startup lifecycle, consent, waiting-room, voice-preflight, transcription-progress, and error markup. The component emits stable CSS class names, but the SDK does not ship an app-wide stylesheet for those classes.

Decision: Updated the game-generation skill and browser-client reference to require generated clients to style the startup classes in `web/src/styles.css` alongside the active game UI.

Tradeoffs: Keeping CSS in generated clients lets each game make the shared startup screens match its visual theme without coupling the SDK to a default design system. It also means generators must remember to style SDK-owned markup, so the skill now lists the relevant classes explicitly.

Follow-up risks: If the SDK later ships default CSS or changes startup class names, the skill and references should be updated together to avoid stale styling instructions.

## 2026-07-13: Voice features use server and SDK surfaces

Context: Generated games need clearer instructions for two voice-related behaviors: agent messages should be spoken when TTS is enabled, and active game screens should expose STT health to participants.

Decision: Updated the game-generation skill and references so agents vocalize text through the server-owned agent response path, leaving synthesis and room-relay publication to `parlando-server`. Also directed generated browser clients to compose SDK widgets such as `MicLevelMeter`, `TranscriptionStatusChip`, and `TranscriptionProgress` from `@coli-saar/parlando-client/react` when STT is enabled.

Tradeoffs: This keeps provider credentials and audio publishing out of generated browser/game code while still giving participants visible feedback about microphone input and ASR state. It also couples generated UI guidance to the current SDK widget names.

Follow-up risks: If the server changes how agent messages trigger TTS, or if the React voice widgets are renamed or replaced, update the skill and references together.

## 2026-07-13: Generated games make completion explicit

Context: Parlando marks sessions complete through the game adapter, but the skill only listed `is_complete` and `completion_summary` without explaining that success and failure both need terminal semantics and durable summary data.

Decision: Updated the game-generation skill and references to require explicit terminal state, `is_complete` returning true for every terminal outcome, and `completion_summary` including success/failure or another analysis-friendly outcome. The browser guidance now treats `session.completed` as a server-driven terminal state.

Tradeoffs: This pushes game authors to model endings up front, which takes a little more design work, but avoids generated games that never notify Parlando of completion or only record successful endings.

Follow-up risks: If Parlando later adds richer built-in completion statuses, update the skill to map game outcomes onto those statuses instead of relying solely on summary fields.

## 2026-07-13: Generated agents respond to relevant participant messages

Context: Human-vs-agent games can involve typed chat or speech transcripts, but a generated agent that only reacts to game actions ignores the main participant interaction channel. LLM-backed behavior may be appropriate for some dialogue-heavy games, but credentials must remain server-side.

Decision: Updated the game-generation skill and agent reference to require `observe_message` handling when participant messages matter, and to ask whether the user can provide LLM provider credentials when scripted behavior is too brittle for the requested agent. The guidance keeps LLM credentials in private server/agent configuration, never in browser code.

Tradeoffs: This adds another design question for agent games, but prevents superficially working agents that fail conversational tasks.

Follow-up risks: If Parlando adds richer conversation history or memory APIs, update the guidance so generated agents use those instead of local bounded memory.

## 2026-07-13: Generated mic meters include visible transform targets

Context: The SDK mic meter updates a child element with `transform: scaleX(...)`. If generated CSS omits block layout, full dimensions, or a visible background on that child, audio levels update but the meter appears frozen.

Decision: Updated the browser styling guidance to require `.mic-meter-track span` or equivalent CSS with `display: block`, `width: 100%`, `height: 100%`, `transform-origin: left center`, and a visible background.

Tradeoffs: This is a small CSS constraint on generated themes, but it preserves freedom over colors, dimensions, and surrounding layout while preventing an easy-to-miss visual bug.

Follow-up risks: If the SDK changes the mic meter DOM, update the selector and required styling guidance together.

## 2026-07-13: Completion is terminal for game-channel input

Context: The reusable server already persisted game-specific completion summaries, but connected clients could still submit game actions, typed chat, or voice transcript messages after completion. The React startup gate also exposed only `completed`, so generated games had no direct access to outcome, win/loss, or score fields carried by the server's `completed` message.

Decision: Treat completion as the final reusable game-progress boundary. After a room reaches `completed`, Parlando rejects participant game-channel input while still allowing lifecycle and operational cleanup such as disconnects and voice diagnostics. The JS client exposes `completionSummary` and no-ops game action/chat sends after completion. Scores, win/loss labels, dyad outcomes, and per-player outcomes remain game-specific fields in the typed completion summary; Parlando persists, exports, broadcasts, and exposes that JSON without interpreting a universal score schema.

Tradeoffs: This keeps the reusable platform flexible across studies while giving clients a reliable terminal state. Post-game conversation now belongs outside the game-channel protocol; studies that need debrief chat should add an explicit non-game surface rather than continuing normal game messages after completion.

Follow-up risks: If Parlando later adds a first-class score or debrief model, map those concepts from completion summaries deliberately instead of inferring them from arbitrary game-specific JSON.

## 2026-08-13: Server-owned audio relay with a final-utterance STT boundary

Context: Parlando needs two-party audio relay, per-speaker transcription, and delivery of completed spoken messages to agents without exposing provider credentials or media infrastructure to browsers and generated games.

Decision: Use one authenticated bidirectional audio WebSocket per human participant, terminated by the Parlando server. Version 1 uses fixed 24 kHz mono PCM16 frames and fans each microphone stream into independent partner-relay and server-side Speechmatics queues. The server authenticates its Speechmatics connection directly, so no Speechmatics credential is returned to the browser. Transcription providers normalize their output to optional partial hypotheses and final utterances. Only a final utterance is persisted as a `voice_transcript` conversation message and delivered to `GameAgent::observe_message`; provider streaming and utterance segmentation remain hidden behind the server-side provider interface. Agent TTS is published through the same room relay and is not transcribed again. The current design is documented in `docs/audio-transport.md`.

Tradeoffs: PCM and WebSockets make the first implementation small, observable, and firewall-friendly, but use more bandwidth than Opus and inherit TCP head-of-line blocking. A browser jitter buffer, bounded queues, and stale-frame dropping are required to keep latency bounded. Fixed 24 kHz audio also requires browser resampling and initially constrains TTS output formats. Speechmatics still receives audio in version 1, but only through the server; a later local provider will be able to reuse the same audio and final-utterance contracts.

Follow-up risks: Browser audio worklets and resampling still need broader Chrome, Firefox, and Safari verification. Initial deployment is deliberately single-process or sticky-session because active audio rooms live in memory. Continue testing long-call stability, Speechmatics end-of-utterance behavior, reconnect deduplication, backpressure behavior, and human-agent TTS playback under realistic network conditions.

## 2026-08-13: Audio-relay migration implemented as a breaking replacement

Context: Parlando has no stable 1.0 voice protocol to preserve. Keeping compatibility sinks, obsolete token endpoints, or browser-owned provider integrations would retain privacy and maintenance costs without serving the current architecture.

Decision: Implement one versioned `/ws/audio/{room_id}` transport carrying fixed 20 ms PCM16 frames, authenticated by an opaque, one-use, one-minute room/participant/role token whose claims remain only in server memory. The in-process room registry relays human audio to the other role and fans the same validated frame into a bounded `TranscriptionProvider` session. Speechmatics is the first provider and runs exclusively on the server. Its streaming partial/final messages are normalized into `FinalTranscriptUtterance`; final utterances are idempotently persisted, broadcast as conversation messages, and delivered to agents as spoken observations. Agent PCM uses the same room registry without entering STT. The browser uses AudioWorklets for 24 kHz capture/resampling and playback with a configurable jitter threshold. All previous browser sinks, native server dependency, public transcript ingestion, temporary-key minting, and provider-specific browser exports are removed.

Tradeoffs: Raw PCM is intentionally less bandwidth-efficient than Opus, and WebSocket/TCP can add head-of-line latency. In exchange, the protocol is small, inspectable, deployable through ordinary HTTPS infrastructure, and keeps media off an uncontrolled relay. Active rooms remain process-local, so multi-instance deployments require sticky routing. Bounded queues favor live audio over guaranteed delivery; browser playback drops stale buffered samples.

Follow-up risks: Verify real-browser resampling and audible quality across Chrome, Firefox, and Safari; add explicit dropped-frame metrics; load-test long calls; and exercise a real final Speechmatics utterance through the public audio WebSocket. A local recognizer should implement the existing provider session/events contract rather than changing the browser or agent boundary.

## 2026-08-13: Space Game smoke-test agent answers from its observation

Context: The deterministic Space Game agent is the quickest manual test target for the new microphone-to-STT-to-agent-to-TTS path, but it previously spoke only after game movements and ignored conversation messages.

Decision: Let the existing back-and-forth agent answer typed and spoken questions about positions, component states, launch readiness, and discovered hints. Answers are derived only from the agent's latest role-specific `SpaceObservation`; the agent never reads the complete `SpaceGameState`. A question produces a message-only `AgentResponse`, so it is persisted normally and spoken by the configured server-side TTS path without causing an unrelated game move.

Tradeoffs: Keyword-based answers are deterministic, fast, and need no additional model or external credentials, but they are intentionally narrower than open-ended natural-language question answering. Unknown questions receive a compact visible-world status summary and suggested topics.

Follow-up risks: If richer dialogue is needed, replace the answer selection behind the same `observe_message`/`AgentResponse` boundary with an LLM-backed agent while continuing to pass only role-safe observations and keeping model credentials server-side.

## 2026-08-13: TTS playback maintains an absolute playout lead

Context: Sending a 20 ms TTS frame and then sleeping for 20 ms makes processing, queueing, timer, and WebSocket overhead accumulate on every frame. Browser playback consumes samples at the hardware clock rate, so this cumulative drift eventually empties the jitter buffer and produces audible gaps. Nearest-neighbor output-rate conversion also adds avoidable roughness.

Decision: Publish the configured jitter-buffer window immediately and schedule every later TTS frame against an absolute deadline derived from the utterance start time. The browser playback queue uses linear interpolation from 24 kHz to the output-device rate, starts behind the configured jitter target, resumes behind a smaller 40 ms buffer after an underrun, trims stale audio, and records each underrun as a voice diagnostic.

Tradeoffs: Prebuffering adds the configured startup latency, normally 100 ms, and raw PCM still inherits TCP head-of-line blocking. Absolute deadlines prevent systematic sender drift, while short recovery buffering avoids turning one late frame into a second full startup delay.

Follow-up risks: Real browsers and networks can still underrun. Monitor `audio_playback_underrun`, verify long synthesized utterances on Chrome, Firefox, and Safari, and adjust `jitter_buffer_ms` only from measured deployment behavior.

## 2026-08-13: Audio Worklets are self-contained and reuse preflight capture

Context: The published TypeScript package points game bundlers at Audio Worklet entry modules with `new URL(..., import.meta.url)`. Vite can emit that entry as a standalone asset or a non-hierarchical data/blob URL, where a nested relative import cannot be resolved. A playback-worklet import failure also exposed that startup retried transport connection after stopping the already approved microphone, making the macOS microphone indicator flash.

Decision: Keep each Audio Worklet entry module self-contained, including the small PCM playback queue in the playback entry. Microphone permission and capture remain a separate preflight phase. Waiting-room startup makes one automatic transport connection using the prepared `MicrophoneInput`; a transport failure tears down only partial Web Audio/WebSocket state and preserves the prepared microphone rather than reacquiring it or entering an automatic retry loop.

Tradeoffs: The playback buffer is tested through the worklet entry module instead of a nested runtime module. A failed initial connection is shown to the participant and does not retry silently; a later explicit reconnect control can retry the transport while still reusing prepared capture if product requirements call for it.

Follow-up risks: Other game bundlers may package worklet URLs differently, so the packed SDK should be tested through at least Vite and one additional consumer build before broad publication.

## 2026-08-13: Audio tests are credential-free and tiered by cost

Context: Audio correctness needs concurrency, provider-protocol, and load coverage, but paid provider calls and long-running load tests are unsuitable for every local build. Repository tests must not require Speechmatics credentials in GitHub or another shared environment.

Decision: Keep all deterministic Rust audio behavior under ordinary `cargo test`, including a local fake-WebSocket contract for the production Speechmatics adapter. Keep TypeScript unit tests in the existing npm suite. Put repeated many-room load coverage in a standalone, optional-feature `audio-stress-tui` binary that runs for ten minutes by default. Its CLI exposes three explicit workload models: production-shaped `realistic` full-duplex 20 ms traffic with periodic agent fan-out, unpaced `saturation` throughput, and deterministic `impaired` cadence with jitter and slow consumers. Workload configuration uses CLI arguments rather than environment variables so invocations are visible, discoverable through `--help`, and reproducible in shell history. The TUI renders only measured workload data: canonical-frame verification, source-specific counts, rolling verified-frame throughput, in-memory latency percentiles, cadence misses, queue pressure and drops, per-room activity, failures, and milestones. Do not add CI workflows or real-provider tests; developers invoke the stress binary manually.

Tradeoffs: Normal Rust tests remain fast and hermetic while still exercising provider message semantics. Browser-specific Worklet behavior is not covered by an automated browser harness. Ratatui and Clap remain behind the `stress-tui` feature, keeping them out of ordinary server builds, while the dedicated binary gives operators a substantially more legible soak test than Cargo's captured test output. Realistic and impaired modes trade headline throughput for a controlled offered load that better resembles two live participants. The impairment schedule is deterministic and process-local, making regressions reproducible but not reproducing independent browser clocks or real network loss. The stress tier can regress if contributors never run its documented command, but it cannot unexpectedly consume paid APIs or lengthen every compile.

Follow-up risks: Run realistic and impaired dashboard modes before high-volume releases; use saturation only for capacity comparisons. The runner exercises the in-process relay rather than real WebSockets, so real-browser checks, reverse-proxy validation, and network-level impairment soaks remain deliberate manual deployment checks rather than repository tests.

## 2026-08-13: Datenschutzprüfung separates reusable platform facts from local studies

Context: Parlando is intended for distribution as a self-hosted research platform. Repeating the same technical platform review at every university would add work without changing the software facts, while controller identity, purpose, recruitment, legal basis, retention, providers, and the planned corpus release remain local study decisions.

Decision: `docs/datenschutz-pruefvorlage.md` asks the Saarland University Data Protection Officer for a reusable, version-bound assessment of Parlando's standard operating envelope. The dossier explains the platform's research purpose, participant and researcher workflows, components, and the necessity of each stored data category before presenting the privacy assessment. It explicitly separates the reusable platform core from experiment code and local operations. The submitted main document is self-contained and does not depend on internal repository documentation. Version-specific evidence and participant texts are submitted only as explicitly named DPO-review annexes with defined contents. Each adopting institution remains the controller and completes a short local adoption sheet. Versioned English participant information and consent items are separate artifacts, so researchers can adapt local facts without rewriting the technical assessment. The default use case includes scientific reuse and creation of a publication-oriented corpus candidate that becomes an anonymous corpus only after removal of mappings and successful content review.

Tradeoffs: A platform assessment reduces duplicate technical review but is not a transferable legal approval. Each institution must still decide and document its study purpose, legal basis, retention, processor arrangements, participant population, and corpus-release procedure. Experiments outside the standard operating envelope need an additional assessment.

Follow-up risks: Attach the generated privacy status report and technical acceptance evidence to the assessment, complete the environment-specific security release gates, and increment the privacy contract version when storage behavior, exports, deletion, participant-facing version evidence, or external participant-data flows change materially.

## 2026-08-13: TTS is outside the participant-data flow by default

Context: The privacy review initially treated ElevenLabs TTS like Speechmatics and an external agent. The current runtime has a materially different boundary: `speak_agent_message` sends only the software agent's generated `message.text` to the TTS provider. It does not send microphone audio, transcripts, participant or session identifiers, original participant messages, or game state.

Decision: Document ElevenLabs as a technical dependency but not as a recipient of participant personal data or a subject of participant consent when agent output is constrained to non-personal text. Make the classification conditional on an experiment-level invariant that the agent does not echo participant statements, names, or derived personal information in its response.

Tradeoffs: This avoids an inaccurate provider and consent description for the intended data flow. It requires agent behavior, especially for future remote or generative agents, to be reviewed and tested rather than assuming every generated response is non-personal.

Follow-up risks: If an agent can reproduce or infer personal content, its generated output may itself be personal data even though ElevenLabs never receives the original participant input. Such an experiment must reclassify the TTS provider, update participant information, and complete the applicable processor, retention, region, and transfer review before collection.

## 2026-08-13: Privacy code roadmap is separated from security work

Context: The DPO review template identifies both privacy-governance gaps and security gaps. Security topics are already tracked elsewhere, while the project also needs a focused view of code changes that would make experiment-specific privacy decisions easier to express, enforce, and evidence.

Decision: Added `docs/datenschutz-code-roadmap.md` as a separate, deliberately small implementation roadmap for fictitious two-person game studies. It is limited to versioning the displayed information/consent text, minimizing identity and voice-diagnostic data, four storage switches that leave the existing message/event schema unchanged, fixed `research` and publication-oriented `corpus` exports for the default reuse workflow, manual participant deletion in the admin UI, narrow checks around hosted speech services, and a privacy status page generated from the running version and configuration. The privacy status is installation-wide, so it gets its own protected `/admin/privacy` route linked from the dashboard's global header rather than an experiment-scoped workspace tab. Remote agents are trusted experiment code by default and require no separate privacy machinery. Automatic deletion and configurable retention jobs are explicitly out of scope. The features use ordinary feature tests; no separate privacy or compliance test suite is required. The security work current at the time was tracked in the now-historical `notes/security-remediation-plan.md`.

Tradeoffs: The reduced scope fits Parlando's actual experiments and is implemented in the normal workflow. Structural reduction removes system identifiers but cannot prove that free dialogue contains no accidentally disclosed real-world detail, so a short content review remains a release step for a public corpus. The design does not attempt to cover clinical studies, intentional collection of special-category data, complex longitudinal identity linkage, or multi-purpose institutional data management.

Follow-up risks: Keep the small roadmap, privacy contract version, and status report aligned with schema, configuration, export, audio, and agent changes. Do not expand them speculatively; add machinery only for a concrete approved experiment that cannot be supported by the six listed changes.

## 2026-08-13: Concise privacy contract implemented in the normal research workflow

Context: The platform assessment needs executable, inspectable behavior without turning Parlando into a general compliance system. The agreed scope is self-hosted university research with fictitious two-person games, default scientific reuse, trusted experiment agents, no automatic deletion, and no change to the existing message/event representation.

Decision: Add Privacy Contract version `1`, four persistence switches, versioned participant-information evidence with a server-computed presentation hash, experiment-specific random human-participant identifiers, descriptive type-and-version agent identifiers, durable dialogue identifiers, and server-side minimization of voice diagnostics. Provide fixed `research`, `corpus`, and `full` admin exports; the dashboard defaults to `research`, while `corpus` is explicitly a content-review-required candidate rather than an anonymous export. Add counted, confirmed manual deletion to human participant cards. Deletion removes consent plus authored messages/transcripts, clears identity fields and the participant identifier, and removes participant references from remaining shared events. The installation-wide privacy status reports the effective configuration and capabilities but never infers DPO approval. Human participant identities are scoped to the experiment, and no automatic retention job is introduced.

Tradeoffs: Random experiment-specific human identifiers and descriptive agent identifiers share one nullable participants-table column. Research and corpus exports use explicit projections, so new internal fields do not appear automatically. Corpus export removes internal identifiers and absolute timestamps but cannot detect identifying content typed or spoken by a participant; publication still requires removal of mappings and content review. Deleting authored action rows would damage the other participant's fictitious shared game, so those rows remain with a `deleted_participant` actor and redacted runtime identifiers.

Follow-up risks: The DSB must still assess the export allowlists, deletion boundary, local participant text, speech-provider flow, and the institution's handling of backups and already released anonymous corpora. Increment the Privacy Contract version when these technical behaviors change. Keep schema migration checks compatible with existing SQLite installations.

## 2026-08-14: Consistent readable identifiers replace participant display names

Context: Researchers need to recognize participants and dialogues in the admin dashboard and to join repeated exports of the same experiment. Export-specific random strings make that unnecessarily difficult. Participant-chosen display names also create an avoidable path for entering real names.

Decision: Assign a three-word random identifier when each human experiment participant or dialogue is created and persist it in SQLite. Human identity rows and recruitment mappings are scoped to one experiment: the same external recruitment identifier produces an independently generated participant identifier in every experiment, while repeated sessions and exports within that experiment reuse it. Human participant identifiers use animal nouns; dialogue identifiers use a disjoint list of place and object nouns, so the identifier kind remains recognizable without `participant-` or `dialogue-` prefixes. Agent participants do not receive random names: their identifier records the durable agent type, implementation name when available, and version, with `unversioned` explicitly marking absent version metadata. Other non-human participant kinds likewise derive their identifier from durable kind and provider identity. Use a small local generator with `rand` and repository-owned word lists instead of adding a name-generator crate: the human/dialogue format is simple, and a local list makes vocabulary review and compatibility explicit. Enforce uniqueness in storage and add a numeric suffix to a colliding descriptive non-human identifier. Split legacy participant rows shared by multiple experiments, migrate legacy `research_*` human identifiers, replace existing non-human random identifiers, and fill sessions without a dialogue identifier. Remove the participant display-name field from the browser flow, protocol, runtime state, database, exports, and admin UI. Retain the readable identifiers unchanged in both `research` and `corpus` exports of the same experiment.

Tradeoffs: Human participant identifiers form part of a pseudonymization while an experiment's recruitment mapping exists; they are not proof of anonymization. Their readability does not make them less random than opaque strings drawn from an equally large space, but retaining them across exports deliberately makes records from the same human participant or dialogue within that experiment joinable. They do not support joining a person across experiments. Agent identifiers are intentionally descriptive provenance rather than anonymizing labels. The current random-name lists provide a finite namespace, so uniqueness still depends on database constraints and retry logic. Changing the lists affects only future human and dialogue identifiers; persisted identifiers remain unchanged except for the deliberate migration of legacy non-human random identifiers.

Follow-up risks: A public `corpus_candidate` still needs the documented content and linkage review before it can be described as anonymous. In particular, an institution that retains a recruitment mapping can still relate a published participant label to its internal participant row. Vocabulary changes should exclude ambiguous, offensive, identifying, or easily confused words and keep the two noun sets disjoint.

## 2026-08-14: Microphone selection follows successful default-device preparation

Context: The startup gate displayed a microphone selector before asking for permission. Browsers can hide device labels until permission is granted, so participants often saw generic choices; after preparation refreshed the labels, the selector was disabled. The control was technically functional but emphasized an uncommon choice at the least useful point in the flow.

Decision: Make `Prepare voice` acquire the browser's default microphone without an exact device constraint. After successful preparation, replace the button with one compact dropdown whose selected value is the active microphone; choosing another concrete input reacquires the stream immediately. Exclude the browser's synthetic `default` alias, remove duplicated device-name text, and strip trailing USB vendor/product identifiers such as `(0d8c:1901)` from participant-facing labels while retaining the browser's raw identifiers internally.

Tradeoffs: The common path has one fewer control and respects the browser/system default. Participants with multiple physical or virtual inputs retain page-level selection without a separate change/cancel mode. A single-input dropdown is disabled because it communicates the active microphone without pretending another choice exists. Device enumeration differs slightly across browsers, so unavailable devices remain absent from the page.

Follow-up risks: Verify the post-permission device list and stream replacement in current Chrome, Firefox, and Safari. If browsers expose duplicate concrete entries, selection may need stable deduplication using both labels and group identifiers.

## 2026-08-14: Startup content is limited to current platform tasks (superseded)

This decision's `study_name` title handling was superseded on 2026-08-16 by deleting participant titles and using functional lifecycle headings. Its restriction of startup content remains useful historical context.

Context: The shared startup gate accepted arbitrary eyebrow, setup-copy, waiting-copy, and game-hint labels. This allowed game clients to place in-game controls, stale instructions, and vague category labels on a platform screen whose immediate jobs are consent, browser preparation, and room entry. The public study configuration already supplies the authoritative game title.

Decision: Remove the generic startup-label API. Resolve the title only from the server's public `study_name`; render no heading or explanatory copy on the initial screen; and keep consent, voice preparation, errors, and room entry as the only initial content. The post-entry waiting state retains concise core-owned readiness text because it describes the participant's current platform state rather than game controls.

Tradeoffs: Game clients can no longer add arbitrary marketing text or premature control instructions to the shared startup surface. Studies that need participant information continue to use structured consent and participant-information fields. Changing the configured game title or the game's visual treatment remains the responsibility of the game/configuration layer.

Follow-up risks: If a future study needs essential pre-room instructions unrelated to consent or browser readiness, add a structured, purpose-specific platform field instead of restoring an unrestricted copy slot.

## 2026-08-14: Platform identity and consent derive from authoritative configuration (partly superseded)

The 2026-08-16 public-API decision supersedes the `study.name` title behavior below. Institution and consent behavior remain current.

Context: The simplified startup title needs to distinguish the Parlando platform, the operating university, and the game without repeating them in one large heading. Consent configuration also had two independent switches: `direct.require_consent` and `direct.consents`, which could disagree about whether declarations were displayed or enforced.

Decision: Add optional `study.institution` configuration and expose it publicly so the shared shell renders `Parlando · <institution>` above the `study.name` game title. Remove `direct.require_consent` from Rust and browser protocols. A non-empty `direct.consents` list now triggers browser rendering and submission, privacy-status reporting, and server enforcement of required declarations; an empty list skips the consent flow. Participant-information version and URL remain optional evidence metadata and do not gate consent or server startup.

Tradeoffs: Institution identity remains optional for installations that do not want it displayed. Games no longer need to repeat Parlando in `study.name`. Consent cannot be temporarily hidden while keeping configured items in place; operators must remove the items from the effective configuration, which makes the visible behavior explicit.

Follow-up risks: Existing configuration overlays must remove `require_consent`; the strict schema now rejects unknown YAML keys. Institutions remain responsible for approved local names, participant-information text, URLs, and consent wording.

## 2026-08-14: One process owns one manually activated experiment

Context: The runtime served one configured experiment, while persistence and the dashboard exposed a second, largely administrative multi-experiment model. The dashboard could create draft or cloned database rows that the running adapter and configuration did not actually serve. Startup also marked an experiment active automatically, leaving no deliberate boundary between deployment and participant intake.

Decision: Treat the configured experiment as the only experiment owned by a server process. On every startup, create or refresh its durable row and reset its lifecycle to `inactive`, even when the database previously recorded `active`. The authenticated dashboard exposes only that experiment and a two-state `inactive`/`active` control. Participant and room creation require `active`; switching to `inactive` closes new intake but does not terminate existing rooms, credentials, or WebSocket connections. Remove experiment-record creation and cloning, multi-experiment selection, UI-only agent option configuration, and obsolete dashboard/direct-start compatibility routes. Retain counted participant-data deletion in session details. First-visitor administrator setup remains intentionally open until the singleton administrator exists.

Tradeoffs: Operators must activate intake after every restart, which adds one deliberate step and prevents an unattended restart from reopening recruitment. Separate experiments now require separate processes and configurations instead of sharing one dashboard. Keeping active sessions alive during deactivation avoids disrupting participants, but deactivation is not an emergency kill switch.

Follow-up risks: Deployment runbooks and health monitoring must distinguish process health from active intake. If an emergency stop is later required, design it as an explicit session-termination operation rather than overloading the intake lifecycle.

## 2026-08-14: Release cleanup makes trust boundaries explicit

Context: The pre-release review found that caller-selected room modes and room ids bypassed the intended matchmaking policy, participant creation could fill durable storage before its in-memory capacity check, expired credentials did not release participant capacity, permissive configuration retained obsolete switches and arbitrary fields, and consent copy was interpreted as HTML. The crate also carried a second in-memory persistence implementation used only by two authentication tests.

Decision: Derive participant identity exclusively from bearer credentials and derive the single `direct` matchmaking mode on the server. Remove public room-code joining. Rate-limit unauthenticated participant creation both per transport peer and process-wide, check capacity before durable insertion, and release participants after their final credential expires and they no longer occupy a room. Make YAML a strict schema, fail on missing environment placeholders, validate provider names, URLs, bounded timing values, consent identifiers, and agent limits before router construction, and remove configuration fields that had no runtime effect. Treat consent bodies as plain text. Persist only a recursively redacted configuration snapshot, including nested token, client-secret, private-key, and suffix-shaped credential keys. Replace the test-only memory persistence backend with SQLite-in-memory tests and record database compatibility migrations in `schema_migrations` so historical transforms execute once. Remove the Space Game's unused direct-room movement and reset actions; reset was not offered by the UI but bypassed ordinary available-action validation when submitted directly.

Tradeoffs: Existing configuration files must delete obsolete keys and rename consent `body_html` to `body`. Manual room sharing is no longer available; every human-human participant enters the same server-owned compatible waiting-room policy. Per-IP limiting depends on the peer address supplied by the trusted server transport, so deployments behind a proxy must preserve an appropriate connection boundary rather than trusting arbitrary forwarded headers. SQLite-backed unit tests are slightly heavier but exercise the production store.

Follow-up risks: A deployment that needs multiple matchmaking pools should add an explicit server-owned policy rather than restoring caller-provided modes. New configuration fields require deliberate schema additions. When adding a database migration, allocate the next version and keep each migration idempotent enough to recover safely from interrupted deployment work.

## 2026-08-15: Tests use production authentication contracts

Context: Unit tests previously relied on conditional middleware branches that accepted a special participant header, treated every unauthenticated request as an administrator, and allowed game WebSockets to authenticate with `participantSessionId`. Those branches were excluded from release binaries, but they preserved obsolete contracts in runtime source and allowed protocol tests to bypass the security boundary they were intended to exercise. The singleton dashboard also retained multi-experiment selection names after its behavior had become singular.

Decision: Remove all conditional authentication behavior from the HTTP and WebSocket handlers. Test helpers now create or sign in a real administrator, retain server-issued participant credentials, attach the same cookie/CSRF/bearer headers as clients, and mint one-use game tickets through the authenticated game-session endpoint. Keep explicit adversarial coverage proving participant identity fields are rejected in authenticated request bodies. Model the dashboard around one `experiment` value without selection state. Move the embedded dashboard document, application tests, storage tests, router construction, and lifecycle policy into focused files while retaining the existing public API.

Tradeoffs: Test setup performs additional Argon2 and HTTP work and maintains credentials in process-local test registries, so the suite is marginally heavier. In return, test and release builds execute the same authentication code, the old participant-id WebSocket path no longer exists anywhere, and the primary implementation files are smaller without introducing new public modules.

Follow-up risks: New authenticated routes must use the shared real-auth test helpers rather than adding conditional shortcuts. The remaining application runtime is still substantial; future extraction should follow stable ownership boundaries instead of creating thin modules that exchange large ad-hoc parameter lists.

## 2026-08-15: One process owns one compiled game and multiple experiments

Context: Making every experiment require a separate operating-system process gives the runtime a simple ownership boundary, but it makes experiment creation depend on deployment and process cleanup. Conversely, one process cannot execute arbitrary independently compiled Rust `GameAdapter` implementations without an umbrella build or a new dynamic plugin interface. Experiments for the same compiled game do not have that type-boundary problem and benefit from one catalogue, one administrator surface, and database-backed configuration editing.

Decision: Supersede the 2026-08-14 process-per-experiment decision. A Parlando game-server process is compiled with exactly one game implementation and manages any number of experiments for that game. Different games use different processes and dashboards; there is no cross-game dashboard, umbrella binary, or dynamic game plugin system. The dashboard identifies the compiled game and semantic version prominently. Every experiment records that exact game version and can activate only under a process with the same version. There is no compatibility matrix: to use current game code, clone an older experiment into a new inactive experiment bound to the running version. The institution is shared game-level configuration. Experiment and game settings move to typed database-backed dashboard forms with immutable experiment revisions; YAML becomes optional interchange rather than the startup source of truth. Process bootstrap remains limited to values needed before the dashboard exists, including an explicit port and durable database location. Participant and WebSocket URLs use the process origin and experiment-scoped paths, so no external URL setting is introduced.

Tradeoffs: A process crash or restart affects every active experiment of that compiled game, which is acceptable for the intended academic deployment scale and avoids per-experiment process supervision. Adding or changing game code still requires building and starting that game's binary, while creating another experiment of the same game does not. Exact version matching is deliberately simple and may require cloning even when two versions happen to be operationally compatible. Moving configuration from files to the database requires revision history, optimistic concurrency, migrations, form schemas, and explicit separation of bootstrap, shared game, secret, and experiment settings.

Follow-up risks: Historical experiment configuration may not deserialize under the current game version, so browsing and export must remain storage-oriented while clone and activation use current validation. The runtime refactor must isolate rooms, credentials, tickets, agents, providers, and lifecycle per experiment without scattering caller-controlled experiment identifiers through request bodies. The complete phased implementation and verification plan is recorded in `notes/multi-experiment-game-server-plan.md`.

Implementation detail: The game host dispatches `/e/{experiment_id}` requests into lazily constructed, typed experiment routers that share only installation storage, administrator authentication, and game settings. Dispatch clears Axum's outer path-capture extensions before child routing so nested room/session paths cannot inherit the host wildcard. Session plans return relative experiment-scoped WebSocket paths, and same-origin checks compare the browser `Origin` with the actual request `Host`; deployments therefore do not need an external-URL bootstrap value. The dashboard renders controls from normalized secret-free configuration values, while every save is authoritatively deserialized and validated by Rust. A fresh installation creates one inactive starter experiment from compiled defaults so administrator setup and experiment creation share the existing authenticated surface; researchers may configure that starter or create a separately named experiment.

## 2026-08-15: Capacity, transport liveness, and durable event volume form one policy

Context: Independent participant, action, WebSocket, audio, and provider limits had accumulated around the most sensitive game and media loops. Several values were implausibly low for real research behavior, while generic message limits could disconnect a healthy one-second heartbeat or audio stream. The browser did not reconnect a quietly lost game connection, and accepted transitions duplicated state snapshots in SQLite. The intended deployment is a modest self-hosted academic game with trusted installed agents and comparatively cheap disk.

Decision: Add one typed, database-backed `capacity` configuration for active sessions, waiting sessions, unattached participant credentials, reserved transcription streams, and a free-disk admission reserve. Admission checks this policy before creating or pairing a session; reconnects replace the previous connection for a room role and do not consume new capacity. Remove generic valid-action and game-message quotas. Human chat uses a burst of twenty and a refill of one message per second; rejected input is retained as bounded, coalesced evidence. Audio validates exact frames and monotonic timing, shapes media that runs implausibly ahead of wall time by dropping it, and records only an aggregate diagnostic when enabled. ASR startup and shutdown are bounded outside the frame loop. Trusted-agent TTS has no application quota, only a sixty-second provider timeout and existing bounded media queues. The client sends a transport-only heartbeat every second, the server declares a game transport dead after ninety seconds, and the client obtains fresh one-use tickets with bounded reconnect backoff for five minutes before reconnecting voice. Heartbeats never touch SQLite or meaningful room activity. Waiting, reconnect, idle, and maximum lifetime defaults are ten minutes, five minutes, thirty minutes, and four hours. Accepted actions store their action, generated events, and resulting state once; unchanged consent and repeated readiness are idempotent. File-backed SQLite pauses only new session admission at a dashboard-configured reserve of 256 MiB by default, and the example Render disk is 10 GB.

Implementation detail: “No generic game-message quota” means no low ceiling that rejects plausible play or disconnects the participant. The transport still drops input above a burst of 200 messages and 100 messages per second thereafter, retains a coalesced rejection aggregate, and leaves the session connected; valid actions have no separate quota.

Tradeoffs: Capacity is intentionally sized for a single modest host rather than pretending to resist a distributed denial-of-service attack. A one-second heartbeat adds small network traffic but no database load and substantially shortens ambiguity around quietly closed browsers. Dropping media that is duplicated, non-monotonic, or far ahead preserves live sessions better than terminating them, but diagnostics are aggregate rather than a complete packet trace. The default disk reserve is deliberately generous because losing an expensive participant session is worse than allocating more storage.

Follow-up risks: Pilot the actual experiment with representative speech and dialogue, measure SQLite and WAL growth per completed session, and adjust dashboard capacity before recruitment. Infrastructure monitoring should alert before the admission reserve is reached. Revisit the defaults only from observed host/provider capacity; do not reintroduce endpoint-local semaphores or environment-variable overrides.

## 2026-08-15: Operational load telemetry stays process-local and heartbeat-safe

Context: The coherent capacity and liveness policy produced useful runtime signals, but they remained scattered across room memory, WebSocket registries, rate shapers, provider queues, and filesystem checks. Researchers need to tell whether a session is connected, merely inactive, approaching cleanup, or competing for a constrained resource during recruitment. Writing one-second heartbeat rows or high-volume audio counters to SQLite would increase contention in the paths the monitoring is meant to protect and would mix operations data into the research record.

Decision: Add one process-local telemetry registry per experiment runtime. Atomic counters cover HTTP concurrency and latency, game messages and heartbeats, accepted and rejected participant input, audio frames and drops, ASR backpressure, reconnect replacement, and trusted-agent TTS. Each role-owned WebSocket keeps atomic connection timestamps; the one-second heartbeat updates those atomics without taking a room lock or writing SQLite. A five-second sampler takes short registry/room snapshots, checks filesystem and SQLite sidecar sizes, and retains 720 samples (one hour). The authenticated `/api/admin/load` endpoint returns the current sample, bounded history, and per-session participant liveness. The dashboard renders capacity gauges, rates, a rolling connection/session chart, per-session health in the main list, separate game/audio ages, meaningful-activity age, and the earliest lifecycle deadline. Game liveness is `live` through five seconds, `delayed` through fifteen seconds, and `stale` thereafter while the socket remains registered; the existing 90-second transport timeout remains authoritative.

Tradeoffs: Operational history resets on restart and is not available in research exports. That is intentional: durable alerting belongs to host monitoring, while this view diagnoses the currently running experiment. Five-second snapshots make brief spikes less visible, but avoid adding a metrics database or locks to audio and game loops. Audio silence is not classified as unhealthy because a participant can be muted; the UI reports connection state and last-frame age instead. Completed sessions retained during reconnect cleanup continue to count against active reserved capacity, matching admission behavior.

Follow-up risks: Pilot the charts under representative browser, audio, ASR, and agent load and tune only the presentation thresholds from evidence. A multi-process deployment would need an external aggregator because each runtime view is intentionally local. If long-term operational retention becomes necessary, export aggregates to a dedicated metrics system rather than extending the research schema or persisting raw heartbeats.

## 2026-08-15: The unscoped root is an administrator entry point

Context: Participant clients now have canonical experiment-scoped URLs under `/e/{experiment_id}/`, so the game-server root no longer identifies a participant experiment. Leaving `/` unmatched produced a generic 404 immediately after a local `make run`, which looked like a failed server and broke the previous useful startup experience. The solo-voice Make target was also removed when configuration moved into the dashboard even though the binary deliberately retains a legacy YAML seeding path for development and migration.

Decision: Redirect `/` to `/admin/experiments`. Unauthenticated administrator HTML requests redirect to `/admin/login`, while administrator APIs retain their existing 401 behavior; authentication is therefore not weakened. Restore `make run-solo-voice` as a compatibility workflow that passes the existing voice YAML through the binary's seeding option and uses the same database and client paths as `make run`. After seeding, ordinary experiment configuration remains database-backed and dashboard-owned.

Tradeoffs: Opening the bare server origin now reveals that an administrator surface exists, which was already evident from the documented `/admin` route and is appropriate for this self-hosted academic operator service. The solo-voice target is intentionally a local development convenience, not a second production configuration system.

Follow-up risks: Keep `/` and the documented local Make targets under regression coverage. Future route or configuration ownership changes must preserve these entry workflows or be proposed explicitly before removal.

## 2026-08-15: One compiled game has one canonical catalogue database

Context: An older local voice configuration selected a second SQLite filename. After the server moved to a multi-experiment catalogue, `make run` consequently displayed only the experiments in the canonical database even though other experiments and sessions still existed in the historical file. Historical configuration revisions can also contain removed fields, so constructing a current runtime merely to browse their stored sessions can fail.

Decision: Every compiled game uses one canonical SQLite catalogue containing all of that game's experiments. Experiment configuration never selects a database. Merge the historical Space Game voice catalogue into `space-game/.local/parlando.sqlite`, preserving experiment and session identifiers and remapping only internal participant row ids where necessary. Provide a reusable collision-checking, transactional catalogue merge command. Route storage-only administrator operations—session browsing, event timelines, export, and participant deletion—through the shared catalogue with an explicit experiment scope; only operational endpoints such as live load and lifecycle actions require a constructible current runtime.

Tradeoffs: Stable identifier collisions abort a merge instead of guessing which record wins. Historical experiments remain browsable and exportable even when their old configuration cannot run under the current schema, while their live telemetry is correctly shown as unavailable. A game still has a separate database from other compiled games, matching its process and dashboard boundary.

Follow-up risks: Future import tooling must retain the same preflight collision checks and atomic verification. Configuration migrations may later make selected historical experiments runnable again, but storage access must not become dependent on that migration.

2026-08-16 correction: The catalogue merge was a one-time local consolidation and is not part of the 0.3 product or migration contract. Remove the `merge_sqlite_catalogues` binary, its internal storage implementation, and its dedicated tests instead of carrying database-import machinery with the reusable runtime. If a concrete import requirement appears later, design it from that requirement rather than preserving this historical tool.

## 2026-08-15: The administrator dashboard mirrors game, experiment, and session ownership (partly superseded)

The 2026-08-16 public-API decision supersedes the participant-facing `study.name` references below. The ownership hierarchy and dashboard layout remain current.

Context: The dashboard accumulated experiment selection, per-experiment load telemetry, shared game settings, privacy status, and session inspection in one undifferentiated workspace. This obscured which data belonged to the compiled game process, one experiment, or one session, and encouraged labels that were richer than the server's actual lifecycle model.

Decision: Give the authenticated administrator UI four game-level areas: Experiments, Operations, Game settings, and Privacy. Keep the experiment catalogue and its Sessions, Export, and Configuration views exclusively inside Experiments. Use full-height structural dividers for the experiment catalogue and session list, with draggable desktop widths. Present durable lifecycle and runtime health as separate session metadata. Render only statuses the server currently owns: experiments are `inactive` or `active`, with the independent `obsolete` flag presented as archived; sessions are `waiting`, `playing`, `completed`, or terminally `expired`. Use the immutable `experiment_id` as the administrator-facing experiment name because participant-facing `study.name` often repeats across experiments; expose `study.name` only as configuration. Package the dashboard's line icons as an inline SVG symbol sprite so navigation and actions remain crisp and accessible without a CDN or separate asset-loading failure mode. Implement status filters as keyboard-operable button/listbox controls because native HTML options cannot consistently render the same styled status dots used elsewhere in the interface.

Tradeoffs: Operations is correctly placed at game scope, but detailed telemetry is still fetched from the selected experiment runtime because the server has no aggregate game-process telemetry contract. The page states that limitation instead of presenting per-experiment counters as process totals. The configuration editor remains schema-agnostic and exposes normalized server keys because the server does not provide dashboard field labels or grouping metadata. The privacy report is now available inside the common dashboard navigation while the existing standalone and download routes remain supported.

Follow-up risks: A complete game-wide Operations view requires an aggregate telemetry endpoint spanning every experiment runtime. A separate researcher-facing experiment name requires a durable field distinct from participant-facing `study.name`. Richer experiment lifecycle states require explicit server transitions and storage semantics before the dashboard can offer them truthfully.

## 2026-08-15: Experiment availability is one lifecycle and operations are game-wide

Context: The multi-experiment host exposed the bootstrap experiment id as `running_experiment_id`, reset every experiment to inactive on process start, stored archive-like state in a separate `obsolete` flag, and returned load and privacy facts from whichever experiment runtime happened to serve the administrator request. The dashboard consequently presented implementation details as experiment state and could not truthfully describe Operations or Privacy as game-wide.

Decision: Use one durable experiment lifecycle with `inactive`, `active`, and `archived`. `active` is the sole administrator-facing availability state: the experiment is valid for the compiled game version, its runtime is constructed on demand, and participant intake is enabled. Internal router construction is not experiment state and is not exposed. A later fail-closed startup decision supersedes preservation of open intake across restarts: every `active` or `testing` experiment now returns to `inactive` when the process starts. Migrate the former `obsolete` flag into `archived` and remove the column and API field. Share process telemetry counters across experiment runtimes, keep a registry of hosted runtime state, and return aggregate current load, history, connections, and experiment-qualified session liveness from the game-wide load endpoint. Build Privacy from every durable experiment configuration and report coverage instead of projecting one selected runtime's configuration. Return the current process version manifest directly rather than deriving it from a selected experiment record.

Tradeoffs: Deactivating an experiment does not destroy an already constructed router or terminate existing rooms; it closes intake, while router presence remains an internal cache detail. Aggregate capacity ceilings are the sum of the hosted runtimes' configured experiment quotas, while the filesystem reserve is the strictest configured reserve. Active experiments with invalid current configuration now fail game startup instead of appearing active but failing on first participant access. Historical configurations are inspected structurally for privacy facts so an old schema does not make the installation-wide report unavailable.

Follow-up risks: Configuration-editor presentation metadata remains deliberately deferred; the server still returns normalized configuration data without labels, grouping, descriptions, or widget hints. If capacity becomes a genuinely shared host budget rather than the sum of experiment quotas, those limits should move into durable game settings with explicit admission arbitration across runtimes.

## 2026-08-15: Testing data is session-scoped and completion is distinct from archival

Context: Researchers need to exercise a participant-ready experiment without allowing those sessions into analysis exports. They also need to distinguish a successfully finished experiment, whose results remain valuable, from an archived experiment that behaves like a reversible deletion. Inferring test data from the experiment's current state would silently reclassify old sessions whenever lifecycle changes.

Decision: Expand the durable lifecycle to `inactive`, `testing`, `active`, `completed`, and `archived`. Both `testing` and `active` open intake and require the current compiled game version; only `active` creates research sessions. Assign immutable purpose `testing` or `research` when participant intake begins, carry it into rooms and consent declarations, refuse to pair participants with different purposes, and persist it on each session. This prevents a waiting test participant from becoming research data merely because the experiment was promoted before room creation. Exclude testing sessions and their events, memberships, consent declarations, and test-only participant identities from full, research, corpus, and single-session exports. Keep completed experiments closed, read-only, and exportable. Treat archived experiments as closed, read-only, hidden from the default catalogue, and non-exportable until restored. Constrain lifecycle transitions so testing can be stopped or promoted, active research can be paused or completed, completed data can be archived, and archival restoration returns to inactive.

Tradeoffs: Test data remains available to administrators for debugging and participant deletion, so archival and testing are not physical erasure. Purpose is duplicated on every session, but that immutable fact makes exports auditable and immune to later lifecycle edits. Existing sessions migrate to `research` because there is no reliable historical signal that they were tests.

Privacy scope: Consent text, consent version, participant information, and collection switches are experiment configuration because they define the contract under which that experiment collects data. The game-level Privacy page remains an aggregate assurance view for shared software, storage, external services, deletion capabilities, and cross-experiment configuration coverage. A dedicated experiment Privacy subview may later present the consent contract more clearly than the generic configuration editor; this decision does not invent editor metadata for it.

Follow-up risks: “Archived” is presently a reversible soft deletion, not physical database deletion. If legal or operational requirements need erasure, add a separately confirmed purge operation with dependency counts and backup policy. Decide whether test data needs a dedicated administrator-only export for debugging; it is intentionally absent from all current exports.

## 2026-08-15: Configuration separates Parlando policy, game YAML, and credentials

Context: The generic configuration editor inferred controls from serialized JSON keys, which produced weak labels and exposed implementation structure rather than a stable administrator interface. Compiled games also had no supported per-experiment configuration namespace. Provider credentials came only from bootstrap configuration, and the dashboard had no safe routine representation or explicit copy workflow for them.

Decision: Render Parlando-owned experiment configuration with a curated dashboard schema: stable sections, labels, descriptions, and control types are maintained with the administrator client, while Rust remains the authoritative deserializer and validator on every save. Add a `game` JSON value to each experiment configuration and edit it as YAML; `GameAdapter::validate_config` validates that value before save, startup, or room creation, and `initial_state_with_config` lets game code consume it without coupling the shared server to a game schema. Reject credential-shaped keys anywhere in game YAML. Space Game currently validates only an empty mapping because it has no game-owned parameters; accepting arbitrary ignored keys would give false confidence. Store experiment secret overrides in a dedicated SQLite table, never in configuration revisions or exports. Known provider keys and namespaced `game.<name>` keys are configured in the Game configuration view. Routine configuration reads return only configured/source metadata. Revealing one value requires an explicit authenticated, CSRF-protected POST; the server reads the selected value from SQLite, or from the server bootstrap fallback, returns it with `Cache-Control: no-store`, and logs only the key. Production deployments must protect that response with HTTPS.

Tradeoffs: A browser cannot read SQLite directly, so a copyable GUI secret necessarily crosses the server-to-browser connection after the explicit reveal action. The value can then appear in browser memory, developer tools, or the clipboard; hiding the input again only shortens casual visual exposure. SQLite values are currently protected by database file permissions and deployment storage controls, not application-level encryption. Secret updates are committed atomically with configuration revisions, but secret values are deliberately not revisioned because revision history would multiply credential copies. Deleting an experiment override exposes an existing server bootstrap fallback rather than deleting that deployment credential.

Follow-up risks: Add role separation or recent-password confirmation before reveal if deployments have multiple administrator trust levels. Application-level encryption at rest requires a master key outside the database plus rotation and recovery policy; do not add reversible encryption whose key sits beside the database. The curated JSON text area for agent structures is an interim control, not game-specific YAML. As Parlando configuration evolves, keep the curated UI metadata and Rust schema under contract tests so omitted fields remain preserved and new fields do not become silently unmaintainable.

## 2026-08-15: Administrator login sessions survive ordinary server restarts

Context: The Argon2id administrator credential already lived in SQLite, but authenticated sessions lived only in a process-local map keyed with a newly generated process pepper. Restarting the game therefore invalidated every browser cookie. The cookie also always carried `Secure`, including loopback HTTP development where browser handling is inconsistent, and sessions expired after only thirty idle minutes or eight absolute hours. This made the local dashboard repeatedly demand credentials even though it uses no cross-site service.

Decision: Persist administrator sessions in SQLite as the SHA-256 digest of a 256-bit random bearer token plus role, CSRF token, creation time, and last-use time. Never store the bearer token itself. Resolve the host-only cookie through that table after every restart, persist idle activity at most once per five minutes, delete the row on logout, and clean expired rows periodically. Use a seven-day idle lifetime and thirty-day absolute lifetime, with the cookie's `Max-Age` matching the absolute bound. Keep the cookie first-party, host-only, `HttpOnly`, and `SameSite=Strict`. Add `Secure` for HTTPS deployments; omit it only for the explicitly supported loopback HTTP development case. The dashboard continues to use ordinary same-origin requests and no local storage, third-party cookies, or cross-site identity flow, so Brave Shields can remain enabled.

Tradeoffs: A stolen live cookie can be used until logout, idle expiry, or absolute expiry, so the longer convenience window increases exposure compared with the former eight-hour cap. The durable table contains only a one-way token digest, but its CSRF token and activity metadata will appear in database backups. Authentication now performs one indexed SQLite read per administrator request and at most one timestamp write per five minutes per active session. Cookies are host-specific: `localhost` and `127.0.0.1` are intentionally different browser origins, so administrators should consistently use one hostname.

Follow-up risks: Add an administrator session-management screen if deployments need to inspect or revoke other browsers. A recent-password challenge remains appropriate for especially sensitive actions such as secret reveal. Private windows, explicit browser data clearing, and Brave's site-specific “forget me when I close this site” behavior will still remove the cookie by user choice and cannot be overridden by the application.

## 2026-08-15: Privacy and hosted-service readiness belong to each experiment

Context: Consent presentation, collection switches, transcription, and synthesized speech can differ between experiments of one game. The dashboard nevertheless placed Privacy beside game-wide Operations and Game settings, where an aggregate report obscured the contract applying to the selected participant population. The configuration editor also required hosted-service credentials during every save, which made it impossible to prepare an incomplete inactive draft, while provider fields were loose text despite a constrained Parlando protocol and known provider contracts.

Decision: Supersede the earlier game-level Privacy navigation decision. Place Privacy inside the selected experiment workspace beside Sessions, Export, and Configuration, and generate the dashboard view plus JSON and Markdown reports from that experiment alone. Retain the aggregate privacy endpoints only for installation-level tooling and portable review records. Keep ordinary configuration validation separate from activation readiness: inactive experiments may save structurally valid voice configuration without credentials, but `testing` and `active` transitions require the effective Speechmatics key, effective ElevenLabs key, and ElevenLabs voice id whenever their corresponding service is enabled. Every process restart returns open experiments to inactive regardless of readiness; runtime-injected provider implementations satisfy credential requirements when intake is deliberately reopened because they own their own setup.

Normalize the complete legacy voice overlay on the server, but expose only meaningful operator choices in the curated editor. Voice transport, speech recognition, and text-to-speech are separate sections because carrying audio, recognizing speech, and synthesizing agent speech are independent functions. Parlando audio protocol version 1 fixes 24 kHz PCM and 20 ms frames, Speechmatics is the only implemented transcription provider, ElevenLabs is the only implemented TTS provider, and TTS output is fixed to `pcm_24000`; these invariants remain validated configuration but are not presented as fake choices. The editor does expose Speechmatics `standard` and `enhanced` model choices and a curated current ElevenLabs model list. Return the current-version experiment configuration after deserialization and default application so older sparse revisions show concrete lifecycle and capacity defaults instead of blank controls. Boolean controls place their labels after the checkbox. Timing labels state their units, and fractional protocol values use locale-independent text controls that require a decimal point so a stored value such as `1.2` is never rendered as `1,2`. Speechmatics and ElevenLabs API keys remain secret-store fields, separate from revisioned configuration and game YAML. A configured but concealed key is rendered as a non-secret bullet mask with a visible configured source and Reveal action; an actually empty input means no key. The warning updates immediately when the draft enables a service without its required credential. A bootstrap private YAML or environment value remains a server-wide fallback and is reported as such; copying it into each experiment is optional, not automatic.

Tradeoffs: An inactive experiment can intentionally remain incomplete, so “saved” does not imply “ready for intake”; the dashboard therefore shows blockers both in the experiment header and configuration form. Automatically deactivating an incomplete experiment on restart favors truthful availability over silently degraded behavior. Curated ElevenLabs model choices can lag new provider releases, although the Rust configuration representation remains string-based so the list can evolve without a database migration. Aggregate privacy reports remain useful to administrators but are no longer presented as the selected experiment's contract.

Follow-up risks: Provider model catalogues change and should be reviewed against official documentation during upgrades. If an experiment needs a different transcription provider or audio protocol, add it as a complete runtime contract rather than making these controls arbitrary text. A future in-process OpenAI-backed agent should declare and consume its own named game secret; the current deterministic Space Game agent uses no model service, while a `remote_grpc` agent owns provider credentials in its external process.

## 2026-08-15: Consent statements use a structured editor

Context: `direct.consents` is a small typed collection with four stable fields, but the dashboard exposed it as raw JSON. That made ordinary consent maintenance depend on JSON syntax and hid the distinction between the stable decision identifier, participant-facing title and statement, and the required flag.

Decision: Render consent as a repeatable structured editor. Each item has explicit identifier, title, plain-text statement, and required controls, plus Add and Remove actions. New items receive a non-conflicting editable identifier and default to required. Reconstruct the same typed `direct.consents` array when saving; Rust continues to enforce non-empty unique identifiers, bounded participant-facing text, and the configuration revision boundary.

Tradeoffs: The editor deliberately supports the current consent schema rather than arbitrary future fields. Removing an item changes the consent presentation in the next immutable configuration revision but does not rewrite historical declarations, whose stored presentation hashes remain evidence of what earlier participants saw.

Follow-up risks: If consent gains localization, grouping, or conditional presentation, extend the typed configuration and structured editor together rather than falling back to raw JSON.

## 2026-08-15: Compiled agents are discoverable and provider connections are game-wide

Context: Human-versus-agent mode was edited as two independent values: a pairing selector and a nullable raw JSON object. Selecting the mode could therefore create a configuration that the server itself rejected, and the dashboard could not know which agent implementations a particular game binary contained. Speechmatics and ElevenLabs connection credentials were also copied into individual experiments even though those clients are shared installation infrastructure; only recognition behavior and synthesized voice choices vary meaningfully by experiment.

Decision: Extend `GameAdapter` with a typed catalogue of compiled agent factories and their structured configuration fields. The dashboard renders that catalogue as an agent selector and atomically constructs `agents.human_vs_agent` with valid runtime limits and factory defaults whenever human-versus-agent pairing is chosen. Space Game advertises its built-in deterministic agent and its remote gRPC adapter, including the latter's endpoint and identity fields. The server remains authoritative: the stored factory id is resolved only by compiled game code and unknown selectors still fail validation.

Move the Speechmatics realtime endpoint and the Speechmatics and ElevenLabs API keys into revisioned game settings and a separate game-wide secret table. Keep transcription enablement, model, language, delay and utterance timing, plus TTS enablement, model and voice, in each experiment. Bootstrap YAML credentials remain installation-wide fallbacks. Schema migration 9 promotes the most recently updated legacy experiment-level value for each known provider key to the game secret table and removes those duplicated provider rows. Game-specific `game.<name>` secrets remain experiment-owned because compiled game behavior may legitimately vary between experiments.

Tradeoffs: A compiled descriptor is deliberately simpler than a general JSON Schema and currently covers the field kinds used by shipped agents. Adding a new factory requires adding its descriptor beside its resolver, but this makes multiple compiled choices explicit and prevents the administrator UI from guessing. Changing game-wide provider settings updates newly constructed runtimes; an already cached experiment runtime retains its constructed provider client until server restart, which the dashboard states explicitly. The migration must choose one value if old experiments contained different provider keys; newest-update-wins is deterministic and preserves the credential most recently maintained by an administrator.

Follow-up risks: Centralize runtime cache invalidation if provider connections must be hot-swapped without restart; doing so must also replace load-sampler registrations rather than leaving stale runtimes. If agent configuration grows nested or secret-bearing fields, evolve the descriptor contract with typed groups and explicit game-secret references instead of reintroducing a raw JSON editor.

## 2026-08-15: Consent presets reproduce the reviewed participant-information package

Context: Re-entering the recurring consent items from the 14 August privacy review was slow and invited wording drift, while treating generic boilerplate as automatically valid legal text would be misleading. The reviewed `docs/consent-items-v1.0.yaml` already distinguishes three core research items from the optional voice/transcription item and identifies the local facts that every deploying institution must supply.

Decision: Offer the four reviewed items as peer, editable dashboard templates. Selecting any template adds one ordinary consent item with `required: true`; voice/transcription has no special runtime semantics. The experiment configuration determines which items exist, and every configured required item must be accepted before participation. Preserve the versioned identifiers and reviewed meaning. Generate complete prose when inserting a template: include the configured information version and institution when available, otherwise use a grammatically complete neutral formulation; identify Speechmatics directly and refer to the processing region described in the participant information. Never place an unresolved template macro on the participant page. Normalize macros saved by the earlier dashboard before administrator display, participant presentation, and presentation hashing, so the evidence hash covers the exact expanded text. Refuse to add a template whose identifier is already present. Present an explicit reminder that the templates assume consent as the legal basis and require local review before data collection. Blank consent items remain available for study-specific declarations.

Tradeoffs: New template text is copied into the experiment draft rather than referenced dynamically, so administrators can make required local changes and the existing presentation hash records the exact result. Legacy macro expansion depends on current game settings until the normalized experiment configuration is saved as a revision; changing the institution before that save changes the presented legacy expansion and therefore its hash, truthfully recording the changed presentation. Future improvements to the documentation do not silently rewrite configured experiments. The UI does not claim that expansion or template selection constitutes legal approval.

Follow-up risks: When the reviewed participant-information package receives a material version update, add a newly versioned template set and retain the prior identifiers for historical clarity. Do not silently change the text behind an existing template identifier.

## 2026-08-15: Secret edits use explicit draft state

Context: The game-settings UI inferred whether a provider key had changed from the visual masked-input placeholder. Normal editing paths such as reveal-then-edit, replacing selected masked text, and deleting a configured value could leave the placeholder marker intact and silently omit the key from the save request.

Decision: Track game-wide provider secret updates in an explicit in-memory map and deletions in an explicit set. User input alone changes that draft state; reveal and hide never count as edits. Clearing a game-owned value schedules deletion, while entering a value cancels deletion. Submit those structures in the same optimistic game-settings transaction and clear them only after success. Confirm the committed transaction with a short accessible status message.

Tradeoffs: Secret draft values remain in browser memory until a successful save or page navigation, which is already true for visible password inputs. Explicit state adds a small amount of client logic but removes dependence on browser-specific password-input event behavior and visual masks.

Follow-up risks: If the settings surface gains navigation guards, include unsaved provider-secret draft state in the dirty-form check without ever logging or serializing it outside the save request.

## 2026-08-16: Reliability tests use deterministic ownership generations and real boundary adapters

Context: The original suites covered the server's main happy paths and pure client helpers, but left asynchronous ownership, bounded queues, file-backed catalogue merge, browser media teardown, rendered startup, worklet framing, and the Python remote-agent SDK largely untested. Reversed promise completion and concurrent factory creation could therefore transfer ownership to stale operations, while malformed media control input and partial initialization failures could escape normal cleanup. Catalogue merge also copied relational rows without preserving immutable session and consent purpose.

Decision: Test concurrency with controlled futures, explicit event delivery, fake clocks, and bounded fan-out rather than sleeps. Treat the newest microphone, game socket, and audio transport generation as the sole owner of mutable client state; coalesce concurrent audio toggles. Exercise SQLite merge against real temporary files and compare the complete imported graph, participant-key remapping, source preservation, dry-run rollback, purpose preservation, and collision atomicity. Test browser code through both pure media primitives and rendered React components using Happy DOM, while fake WebSocket, Web Audio, media-device, and worklet boundaries retain deterministic ordering. Extract capture resampling into a stateful pure converter so quantum-boundary continuity and long-run sample counts are directly assertable; keep processor registration tests as a separate adapter layer. Use Python's standard `unittest` with fake gRPC contexts, real protobuf `Struct` conversion, concurrent capacity reservations, and byte-for-byte proto drift. Wire the three language suites into the root `Makefile` and ratchet JavaScript aggregate statement, branch, function, and line coverage at 80 percent.

Tradeoffs: Happy DOM validates React behavior and browser contracts quickly but does not replace real Chromium microphone permission and audio-device acceptance tests. The 80-percent aggregate gate is an initial ratchet, not the final target in the comprehensive design; startup and protocol methods remain below that eventual per-file target even though high-risk stale-socket and terminal-input cases are now covered. Python tests assume package dependencies are installed rather than creating an environment inside `make test`. The capture resampler retains one boundary sample when interpolation needs the next quantum, adding at most one input-sample of buffering in exchange for eliminating a boundary discontinuity.

Follow-up risks: Add real-browser Playwright lanes, Rust branch coverage and fuzz/property jobs, fault-injecting durable-store tests, historical migration fixtures, and provider/network soak tests before claiming the scheduled P2 gate. Raise per-file client coverage as those lanes land. If remote-agent RPCs are allowed concurrently with shutdown, add per-agent operation ownership so `close` cannot race a callback on the same Python object.

## 2026-08-16: Microphone mute is an explicit reconnect-safe client transport preference

Context: The browser audio sink already stopped sending PCM when disabled, but the public `toggleVoice` operation also initiated a connection and therefore made automatic recovery depend on current state. Muting did not disable the transport-owned cloned track, incoming partner audio could overwrite the participant-facing microphone message, mute failures were not visible in the active game, and a reconnect could report or restore the wrong microphone state. A prior design considered server acknowledgements and synthetic silence because quiet audio had interacted with transport timeout behavior. The current liveness policy instead uses the game-channel heartbeat and explicitly treats quiet audio as potentially muted.

Decision: Separate automatic voice connection from the participant API `setMicrophoneMuted(muted)`. The audio controller owns the desired microphone state, reconciles overlapping requests, and preserves that preference across automatic audio reconnection; a deliberate session reset returns the next session to live by default. Muting both gates outbound PCM and disables the sink-owned cloned `MediaStreamTrack`, while the separately prepared microphone and its local-only level probe remain active. Playback and the audio WebSocket stay connected. The active UI uses an explicit `MicrophoneMuteButton`, reports transition failures, displays a persistent `Microphone muted` or `Microphone live` label, and renders the still-moving meter with a colorless fill while muted. Do not add a server mute-control message, synthetic silence, partner-visible mute presence, or audio-specific heartbeat unless provider testing demonstrates a separate need.

Tradeoffs: Keeping the prepared source active supports immediate unmute and lets participants verify that the selected device still receives sound, but the browser may continue to show its microphone-use indicator while muted. The grayscale meter deliberately represents local input rather than transmitted audio, so the adjacent explicit label carries the privacy state. Server operations cannot distinguish a muted participant from ordinary audio quietness, and transcription receives no frames while muted.

Follow-up risks: Manually test a long mute and muting mid-utterance with each configured transcription provider. If a provider cannot preserve or finalize recognition across that pause, add the smallest provider-aware continuity mechanism without weakening the browser-side guarantee that captured speech does not leave the device while muted. Keep reconnect, rapid-transition, incoming-playback, failure, accessibility, and live-grayscale-meter behavior under automated test.

Implementation detail: The repository test workflow builds the JavaScript package and then runs an otherwise ignored Rust cross-language contract. A line-delimited Node driver instantiates the production `ParlandoAudioSink` with only Web Audio replaced by deterministic boundary doubles, connects through a real WebSocket to the production Rust router, and lets the Rust test command capture, mute, unmute, and playback checks without timing sleeps. The test proves that muted capture reaches neither the partner socket nor the provider-neutral transcription input while partner playback remains active.

Visual refinement: The compact game microphone widget shows the microphone state directly after the device name and repeats it in a small pill at the end of the level row, using green `Live` or red `Muted` rather than transcription readiness, which the startup gate already establishes before entering the game. The level track consumes the remaining row width up to the pill. The level remains animated while muted but uses a faint translucent grayscale treatment so participants can distinguish local input detection from audio transmission. A smaller mute button and subtle header divider keep the `Microphone` heading visually connected to the compact controls.

## 2026-08-16: Public authoring API follows domain boundaries

Context: The initial public Rust and JavaScript APIs exposed runtime composition, complete experiment configuration, provider implementations, storage records, protocol DTOs, duplicate game operations, authoritative state, and generic events. Names such as `GameAdapter`, `GameAgent`, and `ParlandoStartupGate` described implementation position rather than author tasks. The ecosystem is still small enough for one breaking cleanup, but existing games require an explicit migration.

Decision: Make the supported Rust root `Game`, `GameMetadata`, `PlayerRole`, `ActionRejection`, `Server`, and the optional `agent` namespace. Model exactly two roles, independent of whether a human or agent controls them. Combine validation and transition in `Game::apply_action`, combine terminal detection and result in `completion`, and send only role-specific `Observation` to players. Remove generic game events; retain optional role-neutral `transition_metadata` because dashboard inspection and durable analysis logs are legitimate game extensions. Keep experiment configuration, participant information and consent, session limits, pairing, provider configuration, routers, storage, and protocol DTOs internal. Delete the configured participant title instead of moving it into another page configuration: shared lifecycle screens use functional headings, while each game frontend owns game-specific presentation. Register compiled agent factories on `Server` and let their definitions contribute narrow dashboard metadata.

Use actions as the only player operation that changes game state. Treat messages as communication only between the two players; an agent may react by later proposing an action, but message delivery itself never invokes game mechanics. Replace optional response fields with a non-empty action/message enum. Construct an agent asynchronously from role, seed, and agent-owned settings before creating the initial game state, then deliver the first role-safe observation through `Agent::start`. Use `observe_transition` and `observe_message` for later input.

Make the participant protocol versioned and frontend-neutral, with observation, transition, message, presence, completion, and narrow voice capability messages but no authoritative state or generic events. Offer `@coli-saar/parlando-client` and its `ParticipantApp` React layer as optional conveniences, not server dependencies. Use Game, Experiment, and Session consistently; Parlando does not model the broader research Study entity. Publish an explicit no-alias migration guide and an authoritative design-principles document.

Human players and agents receive the same domain information. Participant transitions and agent `observe_transition` callbacks carry the actor, accepted action, and recipient-specific resulting observation. Accepted actions are observable to both roles; a game can use a non-revealing action value such as `SecretAction` when only state consequences should be observed. The shared `Game::Completion` remains the game-specific termination payload sent to human players, delivered to agents through `finish`, and persisted for dashboards and exports; it can carry win/loss, winner, scores, or any other public result without a universal Parlando schema. Role-private terminal facts belong in the final observation. Once completion is known, the agent runtime requests no further decisions: it delivers already-queued observations in order through the terminal transition, calls `finish`, and only then performs resource-lifecycle `shutdown`. The game WebSocket accepts one clean client protocol—`ready`, `action`, `message`, `heartbeat`, and `leave`—and the obsolete WebSocket consent operation is deleted.

Apply the same public-surface discipline to the example game crate. Export only `SpaceGame` and its registered `BackAndForthAgentFactory` from the crate root; keep mechanics modules internal, delete the unused presentation-shaped `SpaceEvent`, and call the terminal domain type `SpaceCompletion` rather than `SpaceSummary`.

Record the participant wire design separately in `notes/message-protocol.md`. The public `docs/client-protocol.md` remains the operational reference, while the note captures why the variants exist, their delivery scopes and ordering, and the information that must not cross the frontend boundary. A final reviewer-oriented revision of all user-facing API documentation remains part of the API-cleanup handoff.

Tradeoffs: Existing games, agents, custom clients, stored configuration imports, and tests must migrate in one step. A narrow API removes convenient access to internal provider and router seams, so a future external integration may require a new focused extension. `transition_metadata` permits useful domain analysis while deliberately excluding presentation-specific events. Parlando's integration fixtures remain available behind an unsupported `internal-tools` feature, but `test_support` is absent from default builds. TypeScript declaration stripping and the package export map keep internal protocol, audio, and test symbols outside the supported type surface, although compiled dependency files necessarily remain inside the package. The supported `ParticipantClient` owns the HTTP lifecycle and `ParticipantApp` owns the complete React transport lifecycle. Its experiment, room, and transport-plan results use JavaScript-style field names. The two transport-plan methods remain public for custom non-React applications, so their `GameSessionPlan` and `AudioSessionPlan` result types are explicitly exported rather than leaking unnamed wire DTOs.

Follow-up risks: The runtime currently equates completion of `Factory::create(...).await` or remote `CreateAgent` with construction readiness. A synchronous model load inside an async implementation can occupy a runtime worker, and initialization timeout, cancellation, retry, and failure presentation are not yet an explicit lifecycle contract. Address this first in the next API change: define readiness, give initialization its own policy rather than reusing a turn timeout, and specify how blocking initialization is isolated. Preserve the chosen ordering—construct agent first, then deliver initial observation—and do not encode readiness as a game message or frontend-specific event.

The participant protocol itself permits a non-React or separately hosted frontend, but the current `Server` builder intentionally stops at same-origin assets or a shared reverse-proxy origin. A browser frontend on a distinct origin still needs a focused public origin-allowlist method. Add that method as future server work without exposing the internal `ServerConfig` or weakening origin checks.

Smaller deferred refinements are deliberately not part of this cleanup: make agent cleanup and first-decision ordering unconditional when readiness is redesigned; replace raw agent settings and browser-shaped string field kinds with typed semantic configuration if real consumers require it; move `internal-tools` behind a physically private crate if the feature boundary becomes burdensome; and introduce status or error codes only when localization or another frontend requires them. English participant status text is acceptable for now.

The server no longer accepts a bootstrap experiment YAML file. The dashboard's revisioned database configuration is the only active experiment source of truth, so the unused include/environment-expansion loader, Space Game YAML examples, and broken `--config` development target were removed. An older YAML file is migration reference material: transfer its values through the dashboard and apply the documented `study` to `session` transformation.

## 2026-08-16: Experiment identity is the immutable catalogue key

Context: Removing the unmodeled Study concept left legacy `study.name` values and made the roles of experiment identity and participant-facing presentation ambiguous. An experiment still needs a unique researcher-selected catalogue identity, but a second experiment name would duplicate that identity. Participants need to know which compiled game they are entering, not an independently configurable study title.

Decision: Let the researcher choose the unique `experiment_id` during experiment creation or cloning and treat it as immutable afterward. Show it as a disabled identity field in the configuration view, and do not add a rename operation or separate experiment-name field. Expose the compiled `GameMetadata.name` in participant-visible experiment information and display it during startup. Migrate legacy configuration by discarding `study.name`, moving only the remaining session-lifecycle fields into `session`, and leaving all stored session records untouched.

Tradeoffs: Correcting an ID after creation requires cloning to a newly chosen ID rather than renaming it. The participant-facing game name is fixed by the compiled game and therefore cannot vary between experiments, which keeps configuration and catalogue identity unambiguous.

Follow-up risks: Any future need for experiment-specific participant branding should be modeled as an explicit presentation requirement rather than reintroducing Study or a generic experiment name. Database migrations must continue to distinguish configuration objects from stored session entities.

Implementation correction: Agent selector labels use the public `AgentDefinition.name` field. The dashboard must not retain the removed `display_name` wire-field spelling, because doing so produces a valid option with an empty visible label.

## 2026-08-16: Process startup does not create catalogue experiments

Context: The multi-experiment router retained a synthetic “primary experiment” so it could reuse an experiment runtime to serve administrator routes. When no bootstrap ID existed, every game-process start generated and persisted a new catalogue row. This contradicted dashboard-owned experiment identity and made process startup look like researcher intent.

Decision: Build a non-persistent administrator shell from compiled defaults and shared installation resources. It serves authentication, catalogue, configuration, monitoring, and creation routes but is never inserted into the experiment catalogue or runtime registry. Construct durable experiment runtimes lazily only for IDs already stored by an explicit authenticated create or clone action. Process startup still closes intake for previously open experiments.

Tradeoffs: A fresh installation has an empty catalogue after administrator setup, so the researcher must choose an experiment ID before configuring or running anything. The administrator shell carries default configuration internally solely as the template for explicit creation; it is not an experiment and has no participant URL or stored revision.

Follow-up risks: Administrator handlers currently share the same application-state type as experiment handlers. The non-persistent shell keeps that implementation reuse contained, but a future server decomposition could introduce a dedicated installation administration state.

Dashboard behavior: An empty catalogue is a normal first-run state. The experiment workspace hides experiment-only headers, lifecycle actions, tabs, and panels, clears any stale selection data, and explains that the catalogue's standard “New experiment” control starts creation. Do not duplicate that action inside the empty workspace. Game-level settings and operations remain available independently.

## 2026-08-16: The public Rust package uses the Parlando product name

Context: Game authors consumed the only public Rust authoring API through the `parlando-server` package and `parlando_server::...` imports. The server suffix described the repository component rather than the author-facing abstraction, made the primary imports less direct, and left the canonical `parlando` crate name unused. The upcoming 0.3.0 release already contains breaking public-API cleanup, and the downstream ecosystem is still small.

Decision: Rename the crates.io package and library crate to `parlando` at version 0.3.0. Games declare `parlando = "0.3.0"` and import `parlando::{Game, Server, ...}`. Release `@coli-saar/parlando-client` as 0.3.0 in the same version train so the Rust runtime, participant protocol, and supported browser API have one documented compatibility target. Keep the repository directory and maintainer-oriented Make targets named `rust-server`, because those describe source layout and release operations rather than the published author API. Update the Space Game's local path dependencies, public documentation, tests, and game-generation guidance in the same change.

Tradeoffs: Existing games must change both their Cargo dependency name and Rust imports, and crates.io treats `parlando` as a new package rather than a rename of the existing publication. The shorter name is less explicit that the runtime executes on the server, but `Server` remains the clear process entry point and the package now reads as Parlando's canonical Rust API.

Follow-up risks: Claim the `parlando` name on crates.io before announcing 0.3.0, publish the Rust and JavaScript packages together, leave existing `parlando-server` releases available for reproducible old builds, and ensure release automation publishes from the renamed manifest. Do not add a compatibility wrapper or split the package and library target names unless a concrete downstream migration need outweighs the additional long-term public surface.

Migration documentation: Use `docs/migrating-0.2-to-0.3.md` as the task-oriented entry point for people and automated migration agents. It coordinates the package updates, migration order, deletion pass, and verification evidence. Keep `docs/migrating-to-clean-api.md` as the authoritative symbol and behavior mapping so the entry guide can remain concise without duplicating a large table that may drift.

Generator correction: Version discovery treats `parlando` and `@coli-saar/parlando-client` as a coordinated pair. During staggered publication of a future release, generated code may use the repository's aligned manifest versions only for explicit release preparation and must report that external registry builds remain blocked. It must not combine different Parlando minor versions or substitute the obsolete `parlando-server` package. Agent guidance also names the actual Rust and Python shutdown hooks and their different combined-response constructors.

Publication status: Both `parlando` and `@coli-saar/parlando-client` 0.3.0 are published. Normal generated games now resolve the coordinated release directly from crates.io and npm; the unpublished-version fallback remains only for explicit future release-preparation work.

## 2026-08-16: Expensive integration tests remain local feature-gated targets

Context: The `mock_client_smoke` and `speechmatics_contract` integration tests exercise expensive internal test infrastructure and should remain runnable by maintainers without being included in the crates.io source package. Explicit `[[test]]` targets in `Cargo.toml` pointed at files excluded by `tests/**`, causing Cargo to warn during packaging.

Decision: Place both integration tests in the repository-only `rust-server-tests` crate, which has `publish = false` and depends on the main crate with `internal-tools` enabled. Keep `tests/**` excluded from the published package and retain the `internal-tools` feature because the tests consume the crate's opt-in `test_support` API. Run the local test crate from the top-level Rust test target.

Tradeoffs: The local tests have their own small manifest and lockfile, so their direct test dependencies must be maintained explicitly. In return, local test execution remains available, the expensive targets are clearly separated from the reusable crate, and the published manifest has no targets referring to omitted files.

Follow-up risks: Add future expensive integration tests to the repository-only crate and keep its dependency versions aligned with the main crate where protocol compatibility matters. If internal test support grows independently from stress tooling, consider splitting the feature into narrower `test-support` and tool-specific features.

## 2026-08-20: Dashboard catalogue refinements share one storage/runtime boundary

Context: Open issues #8, #10, #11, and #12 concern the experimenter dashboard. Notes already exist in experiment storage and API request types, while archival currently traverses experiment runtime construction and therefore fails for structurally outdated stored configurations. The page-title and layout issues are confined to the same embedded dashboard document.

Decision: Plan these issues as one dashboard and experiment-management change. Derive the browser title from compiled game metadata, give catalogue and session-detail columns independent viewport-bounded scrolling, and define the existing `notes` field as raw Markdown without changing its database or API name. Use a locally bundled source editor for Markdown and YAML, with sanitized Markdown preview and server-authoritative YAML validation. Creation may set initial Markdown; cloning never copies notes and always starts the clone with none. Add a one-way, storage-only archive route that never constructs the target experiment runtime. An outdated or structurally invalid experiment exposes only its identity, diagnostic, and Archive action; every other operation, including restoration, requires a valid runtime. The detailed implementation sequence and verification contract live in `notes/dashboard-experiment-management-plan.md`.

Tradeoffs: The bundle spans presentation, editor assets, and routing rather than being a CSS-only patch. In return, it fixes the shared conceptual boundary once: an invalid catalogue entry remains archivable without becoming otherwise operable. Reusing `notes TEXT` avoids an unnecessary database migration; the Markdown contract is enforced by the editor and safe preview behavior. A dedicated one-way archival route adds one narrow internal endpoint but avoids weakening the validation required by every other path.

Follow-up risks: Viewport ownership and source editors must be checked on narrow screens and with keyboard navigation. Markdown preview must remain sanitized, editor dependencies must be local rather than CDN-loaded, and the storage-only route must remain limited to archival while rejecting active, testing, and already archived experiments.

## 2026-08-20: Agent secrets and fingerprints derive from one semantic configuration contract

Context: Open issues #18, #25, and #16 expose three consequences of the same missing abstraction. Agent definitions describe browser widgets rather than value semantics, experiment secrets are loaded but unavailable through public game and agent contexts, and agent identity records do not distinguish different settings for one name/version pair.

Decision: Plan one breaking configuration-contract migration. Replace browser-shaped field kinds with a recursively typed semantic schema and server-side validation. Store secret reference names and their factory-versus-agent-instance delivery purpose in configuration while resolving values into non-serializable, redacting runtime contexts. Give trusted compiled game code all game-owned experiment secrets and give each agent only its explicitly referenced subset. Send only agent-instance secrets in a separate authenticated remote create field; factory/transport credentials remain in the Rust adapter. Compute a versioned SHA-256 fingerprint from canonical factory ID and normalized non-secret settings, including reference names but excluding values, and expose a narrow fingerprint in durable agent identity, administration, and research exports. The detailed migration and security test plan live in `notes/agent-configuration-secrets-fingerprints-plan.md`.

Tradeoffs: This choice requires coordinated Rust API, dashboard, gRPC, Python SDK, game, generator, and documentation changes. It avoids parallel legacy paths and prevents secret values from becoming ordinary JSON settings. Changing a value behind an unchanged reference deliberately leaves the fingerprint unchanged, so the fingerprint identifies configuration rather than deployed model contents or credential versions.

Follow-up risks: Remote secret delivery expands the trusted endpoint boundary and must remain protected by transport and authentication checks. Existing stored settings need an explicit migration or a clear clone-required error. Agent readiness, rejected-action callbacks, arbitrary logging, and RL execution remain separate lifecycle work.

Implementation: The semantic schema is serialized as a tagged `type` plus type-specific properties, which lets the dashboard reuse its established Boolean, numeric, select, URL, and path-assignment control conventions. Objects recurse through the same path model; only secret-reference selectors are new. Rust `SecretValues` and Python `SecretValues` intentionally expose lookup without enumeration or serialization and always redact diagnostics. Remote protocol v4 adds a distinct `agent_instance_secrets` protobuf struct; the bearer token is now a factory-purpose reference and never enters that struct. Canonical fingerprints are computed once in Rust from a version-1 document and persisted in participant metadata and external identity. Included game manifests use workspace paths during repository development so breaking public-contract changes are compiled together; releases replace these with the coordinated published version.

Known migration boundary: Old revisions whose selected settings cannot satisfy the registered semantic definition are rejected with a clone/migration error rather than being silently reinterpreted. Existing participant rows without fingerprints remain readable and are displayed as unavailable. Fingerprints deliberately retain secret reference identifiers while excluding values.

Dashboard fingerprint presentation: Treat the fingerprint as a configuration identity rather than appending a cryptographic fragment to the agent's name. Session participants show a compact eight-hex badge immediately after the agent name; expanding it reveals the agent, version, factory, selectable full fingerprint, and a short explanation of what equality means and that secret values are excluded. Historical rows keep an explicit unavailable label.

Required agent release identity: Make `AgentIdentity.version` a required string and require every `AgentFactory` to implement `identity`; validate that both identity components are non-empty before creating a new automated participant. Version identifies the agent implementation release, while model, prompt, endpoint, and other choices remain configuration fingerprint inputs. Space Game and all Great Tree factories use release version `1`; remote-agent configuration requires an explicit non-empty version. Continue interpreting absent versions only when reading historical participant rows, where `unversioned` remains an honest legacy marker.

Dashboard correction: Secret references remain the persisted semantic contract, but the dashboard does not ask researchers to select or name that indirection for ordinary agent credentials. It derives a stable experiment-secret key from the factory and semantic field path, stores that reference in the revision, and presents the same masked write-only value/reveal/remove interaction used for shared provider credentials. Explicitly named game secrets remain available for game initialization, while agent-owned derived keys are hidden from the generic game-secret list to avoid duplicate controls. This preserves secret/value separation without exposing storage mechanics as configuration UX.

Readiness and persistence correction: The experiment catalogue and lifecycle endpoints compute readiness through the same validation path, including semantic agent settings and referenced-secret availability, so a runnable badge cannot disagree with disabled lifecycle actions. Persistence continues to erase values under credential-shaped keys, but preserves strings that strictly match the non-secret `game.<key>` reference grammar. This exception is deliberately narrow: arbitrary strings under keys such as `api_key` remain redacted, while the identifier needed to resolve a separately stored secret survives revision persistence. Hydration repairs revisions affected by the former redactor only when the expected dashboard-derived secret is still present; it does not invent references to absent credentials or reinterpret non-empty settings.

## 2026-08-20: Great Tree selects local Parlando packages explicitly

Context: Great Tree's convenience runner always installed the published JavaScript client while its Rust manifest had become pinned to the sibling runtime checkout. That made the apparent default workflow mix dependency sources and offered no single way to exercise coordinated unpublished Rust and JavaScript changes.

Decision: Keep both Great Tree manifests on the coordinated published packages by default. Add a `--local` mode to `games/great-tree/run.sh` that first builds the sibling JavaScript package, installs it into the game client without rewriting its manifest or lockfile, and installs the game binary under the repository's `.local` prefix while patching crates.io's `parlando` dependency to the sibling Rust crate through Cargo's command-line configuration. Using `cargo install` mirrors Space Game and avoids rewriting Great Tree's publication-oriented Cargo lockfile. Preserve the same server arguments, port environment variable, and client output path in both modes.

The Great Tree Vite build deduplicates `react` and `react-dom`. Local npm installation resolves the Parlando client through its workspace path, whose development dependencies otherwise provide a second React module and violate React's hook dispatcher invariant. Vite-level deduplication keeps the published package contract unchanged while ensuring both local and published client code render through the application's React instance.

Tradeoffs: Local mode performs extra package builds and installs on every invocation, favoring a predictable one-command development path over incremental speed. Cargo and npm lockfiles continue to describe published dependencies, so production and default runs remain reproducible while local overrides remain explicit and ephemeral.

## 2026-08-20: Agent-agent scheduling remains separate from game semantics

Context: A headless agent-agent session needs a logical heartbeat that selects an agent and calls `respond`. Issue #22 adds that facility without changing existing human-human or human-agent execution. An earlier design considered a shared state-owning `Session`, controller hierarchy, activation objects, and declarative scheduling rules.

Decision: Keep `GameFactory` plus its session-local `Game` as the shared construction and mechanics contracts and do not add a state-owning `Session` domain object. A `SessionRunner<G>` holds the reusable factory, creates one `G` per session, and owns that value and its local `G::State` while it executes one headless session. It uses a small Rust `AgentSchedule<G>` strategy to select the first and subsequent agents. The versioned YAML experiment file selects a compiled schedule by stable kind and ordinary constructor parameters; it does not encode a scheduling transition language. `AgentSchedule::next` receives the existing optional `AgentResponse<G::Action>` plus an optional `ActionRejection`; do not add a near-duplicate `DecisionOutcome` representation. Scheduling controls only when software agents are asked to respond; `Game::apply_action` remains the sole authority on action legality. Runner-owned construction, timeouts, deadlines, decision and message limits, invalid-action limits, and unconditional shutdown remain outside the schedule. `AgentSchedule` is headless-only: issue #22 does not alter live input timing. The detailed design and examples live in `notes/agent-agent-runner-design.md`.

Tradeoffs: The new headless driver is not a universal orchestration abstraction, which keeps the agent-agent heartbeat easy to understand and test and avoids coupling issue #22 to live execution. Some calculation around accepted transitions may initially be duplicated; extract a stateless helper only when concrete duplication warrants it. Adding a new schedule kind requires Rust code and registration, while its selection and small parameters remain convenient in YAML. This avoids maintaining a public scheduling DSL. Reusing `AgentResponse` makes the schedule generic over `G` and exposes typed action and message contents to specialized scheduling code; built-in schedules can ignore those payloads, and this cost is preferable to maintaining a second response classification with invalid representable combinations.

Follow-up risks: The headless driver must conform to the existing public `GameFactory`, `Game`, `AgentFactory`, `Agent`, and `SessionLogger` contracts for construction, callback ordering, completion delivery, logging, and cleanup. Its terminal behavior for `ActionAndMessage` must be tested explicitly; any change to live behavior is separate work. Result artifacts must identify the schedule implementation, version, and parameters for reproducibility. A schedule that repeatedly selects the same agent can fail to make progress, so hard runner limits must remain authoritative and independent of scheduling code.

## 2026-08-20: Privacy records describe the selected experiment's effective configuration

Context: A catalogue of every storage or speech feature that Parlando could use made an
experiment's Privacy tab ambiguous. Marking an inapplicable category as disabled still required
the reader to distinguish platform capabilities from processing that actually applied to the
selected experiment.

Decision: Generate the archival record from the selected experiment definition. Put the effective
participant arrangement, voice, transcription, speech-synthesis, consent, and
participant-information settings in a
plain-language table. Retained-data, non-retention, external-service, corpus, and review statements
are then conditional on those settings: omit transcript storage when transcription is disabled;
omit microphone and browser-voice statements when voice is disabled; omit agent fields when the
experiment is human-human; and name only an external processor when its service is enabled. Product
model, language, voice, build, Git, and configuration-revision details are not privacy documentation
and are omitted. The SQLite section covers retained participant data only; export-specific facts,
including the complete game configuration written to the corpus, live exclusively in the corpus
section. Human participant and session identifiers are described as randomly generated three-word
pseudonyms, including their construction and the fact that they are not derived from names, contact
details, IP addresses, or external participant identifiers. The stable corpus schema may support
more modalities than one experiment uses, but the
human and JSON privacy records describe only the selected experiment's realizable data path.

Tradeoffs: Omitting inapplicable rows makes reports from different experiments structurally
different, which is intentional because they document different processing. The installation-wide
privacy endpoint remains a summary and explicitly directs administrators to an experiment Privacy
tab for an archival record. The generated date and Privacy Contract version remain because they
identify the recorded privacy behavior without turning the report into technical provenance.

Follow-up risks: Every new modality or external processor must add one effective-configuration fact
and use it to gate all downstream claims. Tests must cover both enabled and disabled configurations
so a generic platform capability cannot leak back into an experiment-specific record.

## 2026-08-20: YAML experiments separate session execution from learning

Context: Issue #22 requires readable experiment configuration, batch evaluation, tournaments, resumable execution, and optional reinforcement-learning self-play. The design must assign two independently configured agents to seats, run training and held-out scenarios, update a learner after a fixed number of epochs, and validate selected checkpoints without making checkpoint storage or observation encoding runner concerns.

Decision: Make a versioned, strictly validated YAML file the public specification for an agent-agent experiment. Named agents are defined separately and assigned independently to the two seats for each phase; scenario lists contain game configurations, seeds, and repetitions. `ExperimentRunner` expands that file into deterministic `SessionPlan` values, runs `SessionRunner` with bounded concurrency, finalizes result, trace, and trajectory artifacts, and, in RL mode, owns the epoch, checkpoint-update, and held-out-validation cadence. Do not add `SessionRecorder`; each bounded `SessionResult` carries compact output and optional artifact references. `RLAgent<G>` owns observation and action encoding, inference, optimization, and resolution and persistence of opaque semantically immutable checkpoint IDs. Keep reward outside both `Game` and `RLAgent`: a selected compiled `RewardFunction<G>` may inspect authoritative transitions inside `SessionRunner` but releases only per-role numeric rewards. The initial RL path treats accepted actions as environment steps and supports action-only learning agents. The full design lives in `notes/agent-agent-runner-design.md`.

Tradeoffs: The YAML schema becomes a compatibility surface that requires versioning, strict validation, and migration discipline. Per-session trajectory buffering is simple and bounded by hard limits but may later need an internal streaming sink. Opaque checkpoint IDs allow in-memory, file-backed, or remote learners and keep save cadence inside `RLAgent`; cross-process resume is possible only when that implementation can resolve a recorded ID. Keeping rewards in a separate trait permits multiple learning objectives per game but requires the experiment to select and record one. Action-only RL postpones communication-policy step and encoding semantics.

Follow-up risks: Canonical YAML normalization, run and plan identity schemas, status codes, shard sizing, training-update idempotency, and the first RL validation game remain implementation choices. Version 1 requires `epochs` to be divisible by `checkpoint_every_epochs`, avoiding an implicit final partial update. Partial shards must never be treated as finalized, callback-level transport recovery must not duplicate a completed logical callback, and cleanup must release agents and provider permits on every exit path. The headless driver needs contract tests for accepted transitions, observations, completion, and terminal combined-response behavior; no live runtime change is part of issue #22.

## 2026-08-20: Experiment exports use dashboard identifiers and deletion removes shared sessions

Context: Issues #26 and #27 exposed that the implemented research and corpus projections do not satisfy the fixed export boundary described by Privacy Contract version 1. Nested runtime envelopes can carry internal identifiers into research output, corpus output can retain absolute timestamps and does not provide a coherent representation of actual session data, and participant deletion currently retains the participant and shared sessions as redacted database rows. The dashboard also offers selected-session export even though the required research operation is an experiment-wide dataset.

Decision: Replace the current projections with one code-owned, versioned, experiment-level `corpus` data product. Its single experiment object contains experiment and game metadata, non-secret condition configuration, a catalogue of every participant referenced by the export, and every non-testing session. Each session carries its own metadata and completion plus exactly both participant-to-role assignments and an ordered typed event list. Participant metadata is defined at experiment level and sessions reference it by the durable dashboard participant ID. The product preserves meaningful actions, communication, game configuration and state, transition metadata, and outcomes while excluding operational, recruitment, consent, runtime, and credential data. Database wall clocks are converted explicitly: `sessions.started_at` is the session event origin, event offsets are checked subtractions from it, waiting time is `started_at - created_at`, and completed-session duration is `completed_at - started_at`. Event indices remain authoritative for order. Missing origins or negative intervals make export fail; they are never clamped, saturated, assigned a substitute origin, or emitted as absolute timestamps. It remains a `corpus_candidate`: the institution must clean explicit identifying content from utterances, remove external linkage, assess rare trajectories, and document the release decision before distribution. Retain the dashboard Data variant selector with one Corpus candidate entry and remove selected-session, status, and event-type scopes. Export the readable three-word dashboard session name, currently stored as `sessions.dialogue_id`, under `session_id`; do not export the numeric database key or a second dialogue identifier. Export durable dashboard participant labels as participant IDs. Retain JSON, YAML, and CSV as encodings of the same logical `parlando.corpus.v1` scheme. JSON is canonical, YAML must round-trip to the same value, and CSV keeps a documented `table,row_json` representation with manifest, experiment, and nested-session rows so schema, experiment metadata, both participants, and events are not lost. Delete the four `privacy.store_*` switches throughout the product rather than deprecating them. Do not ship a migration or compatibility path for these keys. Back up each existing workspace SQLite database, destructively remove the keys from `experiments.config_json` and every `experiment_config_revisions.config_json` document, advance those documents to Privacy Contract 2, and verify both tables and SQLite integrity. The product source contains no switch-name constants, migration helper, fallback parser, or compatibility test. New inputs containing them fail unknown-field validation. Collection follows enabled modality; genuinely absent historical session data remain absent rather than being fabricated. This switch-removal decision supersedes the earlier configurable-retention decision recorded on 2026-08-15. Replace manually maintained Privacy-tab prose with the scheme descriptor and an inventory computed from actual database contents. The descriptor states the experiment structure and exact relative-time derivation. The tab and generated Markdown/JSON reports render the same data, with tests requiring agreement with the downloadable JSON Schema, schema digest, all three encodings, and direct database counts. Participant deletion physically deletes the participant and every session in which the participant appeared, including the other participant's data in those sessions. Delete dependent consent, event, and membership rows transactionally before deleting sessions and the participant; do not introduce nullable references or tombstones. The detailed work and verification sequence live in `notes/experiment-export-privacy-remediation-plan.md`.

Tradeoffs: A single product does not preserve exact wall-clock timestamps or operational diagnostics for later controlled analysis. Relative timing preserves behavioral intervals without retaining collection dates, but integrity defects that the old saturating subtraction concealed now block export and require data repair or an explicit future policy. The experiment-level participant catalogue avoids repeating participant metadata but requires consumers to resolve session references. That loss and indirection are accepted because the current concrete task is corpus distribution, and adding an unreviewed broader export would weaken the clarity of the privacy boundary. Always preserving the semantic trace increases storage compared with a switch-disabled experiment but gives every new session a predictable corpus meaning. Destructively rewriting revision JSON means historical configuration snapshots no longer reproduce the removed retention choices; this is intentional because those choices are not a supported product concept. Previously omitted session content cannot be recovered. Readable dashboard IDs remain pseudonymous and stable, while numeric database keys stay internal. YAML naturally represents the nested document; CSV's JSON-valued rows are lossless but are not a universal flat analytics table. Whole-session deletion sacrifices the other participant's contribution but provides a clear database boundary. Fixed code-owned schemas require a release for structural changes. Actual database inventory makes historical omissions visible but requires counted queries and per-experiment reporting.

Follow-up risks: Participant-authored utterances are the primary residual identification risk. Game-owned configuration, action, state, transition metadata, and completion can also contain experiment-specific personal or sensitive content, so local review cannot be limited mechanically to the `text` field. CSV consumers must understand the documented row encoding. The deletion preview must state the effect on other participants, and active sessions must not race deletion. SQLite secure-deletion, WAL, checkpoint, and backup behavior must be tested without overstating byte-level erasure. Existing 0.3.0 exports should be regenerated. Update the Privacy Contract version and participant information before operational use and provide revised technical acceptance evidence.

## 2026-08-20: The Privacy tab produces an archival experiment record

Context: The first schema-backed Privacy tab exposed assessment state, hashes, implementation flags, and separate diagnostic tables. Those facts were technically useful but obscured the tab's actual task: give an experimenter a stable description of data processing to retain beside the collected data.

Decision: Present one experiment-specific data-processing record in the dashboard and make Markdown its primary download. Organize the record as a self-contained lifecycle: purpose, SQLite storage and access, retention, each retained data category and its purpose, meaningful non-retention guarantees, external speech processing, the corpus export, consent evidence, deletion, and institutional obligations. Use succinct tables for comparable facts in both HTML and Markdown, with prose for scope, derivation, and limitations. Use plain descriptions in the human report and keep exact schema field notation in JSON and the downloadable schema. Internal database labels such as `identity_provider` must be translated and defined, not exposed as undefined terms. Describe stable processing rules from the effective experiment configuration; do not report corpus size or counts of participants, sessions, actions, messages, transcripts, or other records, and do not scan session data to build the report. Represent corpus selection as the projector implements it: a new document built only from a section-by-section output-field allowlist, not a database-shaped document whose unwanted columns are subsequently deleted. Qualify timing claims because Parlando-owned wall clocks become relative intervals, while opaque game-owned configuration, action, transition, state, and completion values are included as supplied and may contain their own dates or identifiers. State verified privacy benefits with their boundaries: participant IP addresses are not written to SQLite or the corpus by Parlando, raw microphone audio is streamed but not stored, interim recognition text and browser voice diagnostics are not retained, and infrastructure or provider logs remain the institution's responsibility. Keep machine provenance such as the schema digest and Git revision in the downloadable record and JSON, but subordinate it to the human explanation. The record states which facts Parlando enforces and explicitly tells experimenters to supplement controller identity, legal basis, retention, hosting, encryption, backup, access-log, and processor-agreement details. JSON remains available for machine processing, and the export schema remains downloadable from the corpus section rather than appearing as a primary tab action. This decision supersedes the database-inventory presentation proposed in the preceding export decision.

Configuration-dependent claims must describe the selected experiment, not merely platform capability. With voice disabled, the record states that Parlando does not receive, relay, store, or send microphone audio and does not name a transcription provider. With voice enabled but transcription disabled, it records live participant-to-participant relay and explicitly states that no provider receives audio. Only an enabled transcription configuration may name its actual provider and describe provider streaming. The same rule applies to transcript storage, browser diagnostics, external-service tables, and provider-status events. Warnings about recruitment mappings or other integrations that the runtime does not implement are prohibited from the generated report.

The generated content-review instruction is deliberately limited to participant-authored messages and final voice transcripts: experimenters remove explicit identifying information from those utterances before sharing. The report does not characterize game configuration, actions, state, completion, or uncommon behavior as speculative privacy hazards. It states that game-owned values are included unchanged without manufacturing an additional review obligation. This narrower instruction supersedes broader warnings in the preceding export decision.

Tradeoffs: The dashboard no longer acts as a dense implementation diagnostics view. The JSON retains the underlying rules for audits and tooling, while the Markdown document is deliberately optimized for reading and long-term storage. The record remains stable as data accumulates, but it is not a corpus inventory or collection-progress report. Institutional facts cannot be inferred safely by Parlando and therefore remain an explicit completion obligation rather than empty generated fields that could be mistaken for documentation.

Follow-up risks: The generated record captures the effective experiment configuration at generation time, so experimenters must generate and retain it at the relevant archival point. Future export-schema changes must update the shared status model, Markdown renderer, dashboard preview, and their agreement tests together.

## 2026-08-20: Session events and speech share one authoritative game clock

Context: Session events were stored as RFC3339 wall-clock timestamps and converted to
`time_from_session_start_ms` only during corpus export. Speechmatics utterance boundaries were
stored separately as offsets from each transcription stream. Similar-looking numbers therefore had
different origins, could not be compared after an audio reconnect, and made the dashboard display
long locale-formatted dates for a within-game timeline.

Decision: Use `sessions.started_at` as the authoritative origin for each game instance and store
every session event as signed integer `game_time_ms`. The running transition sets that wall-clock
anchor and atomically rebases lifecycle events recorded while the room was waiting, yielding
negative pre-game coordinates and non-negative in-game coordinates. New transcription sessions
capture the server timestamp of their first accepted PCM frame; provider-relative utterance
boundaries are translated through that stream origin into `start_game_time_ms` and
`end_game_time_ms` before persistence. Persisted conversation payloads omit their live-protocol
wall-clock field. Corpus events expose `game_time_ms`, utterance timing declares `game_clock` as its
origin, and the dashboard renders compact `m:ss.mmm` coordinates in a fixed column on the left.

Tradeoffs: `started_at` remains one absolute anchor per session so relative values can be audited and
new audio-stream origins can be mapped. Operational timestamps outside the session event stream—such
as administrator sessions, consent declarations, catalogue creation, and liveness state—remain wall
clock values because they have operational or evidentiary meaning outside a running game. Waiting-room
events briefly contain Unix milliseconds until the atomic start transition rebases them; they are not
eligible for corpus export while the session is waiting. Server receipt of the first accepted audio
frame is the shared-clock bridge, so network capture latency is included consistently and browser
clocks never become authoritative.

Follow-up risks: Dropped PCM before provider ingestion can compress a provider's media timeline;
backpressure handling should eventually insert equivalent silence or carry a discontinuity mapping.
Historical databases did not retain transcription-stream origins, so destructive conversion can
rebase event timestamps exactly but can only reinterpret legacy utterance offsets under their old
near-session-start assumption. Backups must be retained for any study needing to revisit that
limitation.

## 2026-08-20: Runtime stress uses real transports and temporary file-backed SQLite

Context: Parlando's existing stress dashboard exercises the process-local audio relay, while live-server integration tests cover only a small number of human-human, human-agent, and remote-agent sessions. Neither layer sustains concurrent participant admission, HTTP and game-WebSocket traffic, SQLite persistence, connection churn, and live-agent cleanup. The planned headless agent-agent runner deliberately bypasses those live-runtime boundaries and therefore cannot supply this coverage.

Decision: Plan a feature-gated `runtime-stress` binary in the non-published server-test package. It starts the production router on loopback and drives public participant, consent, room, ticket, game-WebSocket, audio-WebSocket, message, action, and production-cadence PCM paths with a deterministic fixture game and local scripted agents. Human-human and human-agent workloads run separately at first. The combined acceptance profile enables audio for every human participant, uses canonical 973-byte PCM frames every 20 ms, and drives production transcription and streaming-TTS adapters against credential-free local network peers with configurable latency and failure distributions. Human-agent agent speech traverses the normal audio publisher; an agent is not represented as a browser audio socket. Churn distinguishes clean close, abrupt loss, explicit leave, reconnect, and connection replacement for both transports; agent impairment follows the real factory, callback, bounded-inbox, completion, and shutdown model. The default acceptance preset lasts approximately ten minutes: one minute of ramp, four minutes steady, three minutes of churn, and two minutes of drain. Embedded runs compress reconnect and idle lifecycle bounds so cleanup is observable during the drain phase without accelerating ordinary message, action, audio, persistence, or callback latency.

Every embedded run uses a real temporary file-backed SQLite database through the normal WAL, busy-timeout, connection-pool, and synchronous settings. The runner samples main, WAL, and shared-memory file sizes, preserves the database by option and automatically on failure, checkpoints after shutdown, and validates all synthetic session events directly. Correctness, persistence ordering, stale game and audio connection exclusion, terminal state, frame integrity, transcript and TTS attribution, cross-room isolation, and resource cleanup are absolute gates. Hardware-dependent latency and capacity thresholds are set only after reference runs. Capacity is reported as a workload-specific operating envelope. The normal reproducible estimate uses 100% human audio with local ASR and TTS peers; the most realistic study estimate requires an explicitly authorized calibration against the actual game, host, proxy, TLS endpoint, agents, and providers with stated load and cost bounds. That calibration uses a game-specific workload adapter for representative legal participant behavior rather than relabelling fixture-game results. Extract the existing `audio-stress-tui` terminal lifecycle, controls, progress gauge, metric panels, sparkline, room heatmap, event panel, color rules, terminal restoration, and final summary into one feature-gated dashboard module used by both stress binaries. `runtime-stress` supplies runtime-specific measured snapshots to the same TUI; it does not maintain a lookalike implementation. Interactive TUI mode is the default, while `--no-tui` preserves the same reports and exit semantics for automation. The full workload, measurement, database-volume, reporting, dashboard, and implementation plan lives in `notes/runtime-stress-test-plan.md`.

Tradeoffs: A Rust loopback runner exercises production game and audio protocols, provider adapters, and persistence without the cost and nondeterminism of hundreds of browser processes or default paid-provider calls. Local peers reproduce protocol, queue, task, and pacing costs but not external quotas or regional latency. It does not reproduce proxy, TLS, browser scheduling, or wide-area-network behavior. A small fixture game isolates runtime overhead but may underrepresent a deployed game's computation and serialized state; configurable state padding and the opt-in deployment calibration cover those dimensions. Compressed lifecycle limits verify ordering and cleanup within ten minutes but do not replace exact timer-boundary tests or occasional longer soaks. Sharing the TUI requires a small presentation snapshot abstraction, but it gives both runners identical controls, status colors, terminal cleanup, and report-facing semantics. Runtime-specific panels remain data adapters rather than branches inside the workload engine.

Follow-up risks: The generator can become the bottleneck at continuous 20 ms cadence and must report its own scheduler and resource pressure. Internal cleanup snapshots must remain count-only, exclude participant content and credentials, and stay behind an internal feature. Reference thresholds must record hardware, audio participation, provider tier, and workload parameters, and changes must not hide regressions by silently reducing offered load. Real-provider calibration must require explicit authorization and hard session, audio-minute, request, and cost bounds.

## 2026-08-20: Runtime stress phases control workload behavior

Context: A duration label alone would not reproduce the connection lifecycle pressure that the
runtime test is intended to measure, especially in short developer runs. The same runner also needs
credential-free audio processing while retaining the production runtime queues and attribution.

Decision: Resolve every preset and explicit duration into four fixed 1:4:3:2 phases. Ramp paces
public participant admission, steady sustains production-cadence traffic, churn replaces both real
WebSockets for a deterministic quarter of sessions with fresh one-use tickets, and drain sends a
terminal action before closing sockets. Scale the phase boundaries with the requested duration,
with ten seconds as the minimum useful override. Enable transcription in both pairing modes through
a deterministic asynchronous provider override. In human-agent mode, instantiate a real
session-local agent and use a deterministic 24 kHz mono TTS provider so synthesized messages travel
through the normal room audio publisher. Record resolved configuration, correctness counters,
provider activity, database row counts, and physical SQLite/WAL/SHM sizes in JSON. Treat a healthy
target as a workload-specific lower bound, not an inferred universal maximum.

Tradeoffs: Fixed proportions make short runs comparable and keep the preferred multi-phase shape,
but very short overrides compress realistic dwell time and are only smoke checks. Local providers
exercise Parlando's concurrency and persistence paths without external cost, while omitting real
provider quotas, network variance, and model compute. Reconnecting both transports together covers
the common lifecycle but does not yet replace the dedicated race tests for every independent stale
generation ordering.

Follow-up risks: Deployment capacity still requires repeated acceptance runs on the intended host,
representative game logic, explicit resource and latency thresholds, and an authorized external
provider/proxy calibration. A future impaired profile can add independently churned transports and
controlled provider failures without weakening the deterministic acceptance gate.

## 2026-08-21: Experiment revisions own exact provider destinations

Context: `ExperimentConfig` exposed a Speechmatics realtime URL, but persistence removed it and
runtime hydration replaced it with a game-wide value. The game-wide default could therefore win
over an accepted experiment value. ElevenLabs used a hardcoded service origin. Provider keys also
had an implicit process-environment fallback. These paths made the effective external data
destination differ from the immutable experiment record and could make consent or privacy evidence
describe settings other than those used by an already constructed runtime.

Decision: Supersede the 2026-08-15 decision that made the Speechmatics endpoint game-wide. Keep
Speechmatics and ElevenLabs endpoint values in every experiment revision. Game settings hold only
creation defaults; creating or saving a sparse configuration materializes those defaults without
overwriting an explicit experiment value. Changing a game default never rewrites an existing
experiment. Add `tts.base_url`, make the ElevenLabs client consume it, migrate every current and
historical revision to explicit endpoints, and report exact enabled destinations in the privacy
record. Keep provider credentials exclusively in the explicit game secret table. Remove
`SPEECHMATICS_API_KEY` and `ELEVENLABS_API_KEY` environment lookup and fail activation when an
enabled service has no stored credential. Mark experiment identity, server topology, database
location, and provider credential fields as non-deserializable experiment data so submitted JSON is
rejected instead of accepted and overwritten.

Tradeoffs: Endpoint changes now require a new experiment revision, which is intentional because
they change the external processing boundary. Existing installations receive the endpoint that was
effective at migration time, including historical revisions whose discarded value cannot be
recovered. Credentials remain shared by all experiments in one game process; deployments needing
different provider accounts must deliberately add a future experiment-secret contract rather than
receiving another fallback chain. Internal test constructors may still place credentials directly
in an in-memory typed configuration, but serialization omits and rejects those fields.

Follow-up risks: Endpoint syntax validation establishes a WebSocket destination but cannot prove
provider identity, jurisdiction, retention policy, or contractual terms. Operators must review
custom and proxy endpoints. Any future per-experiment provider credential feature must use explicit
secret ownership and must not silently fall back to game or environment values.
# Runtime stress harness uses production process and protocol boundaries

Context: the previous runtime stress tool built Parlando's internal router and
injected local provider traits. That made it inexpensive, but it bypassed the
public server builder, production provider clients, process resource boundaries,
and deployment-style cleanup.

Decision: `cargo stress` now invokes one executable whose public supervisor
starts hidden `__server` and `__providers` modes through `current_exe`. The
server mode uses `Server<StressGame>` and the provider mode implements the
Speechmatics and ElevenLabs WebSocket contracts on an ephemeral loopback
listener. Provider endpoints are written atomically and stored in the immutable
experiment configuration. The public CLI exposes only workload scale, duration,
pairing, presentation, artifact retention, output root, and deterministic seed.
A repository Cargo alias avoids creating a root workspace.

Tradeoffs: a self-contained protocol peer measures Parlando under a fixed local
latency/cost profile, not provider capacity. The supervisor and peers share the
host, so results are conservative and are fixture-specific lower bounds.
Process groups and descriptor preflight are Unix-complete; non-Unix builds use
the portable child APIs and report descriptor values as unavailable.

The public server runner installs Axum graceful shutdown for SIGINT and SIGTERM.
This is production lifecycle behavior rather than a harness-only hook, and lets
deployments stop accepting new connections while in-flight work drains.

Capacity telemetry and admission count only waiting/running rooms. Completed or
abandoned rooms remain in memory for diagnostics but no longer reserve active
session or Speechmatics capacity. The stress harness exposed the previous
coupling because all transports and agents drained while the reported ASR
reservation remained nonzero indefinitely.

The stress dashboard derives rolling throughput from the same cumulative
counters written to the JSON report. It derives one-second rates and retains
one minute of actions, messages, PCM frames, and TTS publication rates;
presentation never owns a second measurement path. The shared series selector
uses Tab to switch graphs without changing workload scheduling.

Follow-up risks: capacity claims require repeated host-specific runs. Provider
contract fixtures must be updated deliberately when the production adapters'
wire protocol changes.

## 2026-08-21: The stress dashboard is a supervised workload peer

Context: the stress supervisor previously joined the asynchronous workload before inspecting the
dashboard thread. If terminal rendering failed or panicked at higher session counts, the visible UI
disappeared while the workload and child processes continued running. The room heatmap also built
rows for every session even when the terminal could display only a bounded viewport.

Decision: supervise workload and dashboard completion concurrently. An unexpected dashboard exit
sets the shared cancellation flag, waits for workload shutdown, restores the terminal after caught
rendering panics, and returns through the ordinary child cleanup and report path. Bound room tiles
to the drawable viewport, use one cell per room, and state how many rooms are off-screen.

Tradeoffs: presentation failure now ends a non-headless stress run instead of continuing to gather
invisible results. Headless operation remains independent of terminal rendering. A narrow terminal
may hide some room tiles, but the title discloses the count and the JSON report remains authoritative.

Follow-up risks: terminal initialization itself occurs before panic containment because restoration
requires an initialized terminal. If ratatui initialization can panic on a supported terminal, the
thread join still reports it and triggers cleanup, but restoration may require the user's shell reset.

Neutral dashboard values use the terminal's default foreground rather than ANSI white. This keeps
configuration and process metadata legible on both light and dark terminal backgrounds; semantic
health colors remain explicit. Metric rows reserve a two-column label/value gap so a full-width
label such as `Messages/s / actions/s` cannot visually run into its measurement.

The workload treats session completion as an acknowledged protocol transition, not a successful
socket write. During drain it keeps both participant game transports connected until the server
broadcasts `completed`. Temporary `transcription_not_ready` and `players_not_ready` responses are
retried for at most ten seconds; expiry becomes a room-specific capacity failure. This prevents the
harness from racing its own peer disconnect against action evaluation while retaining a bounded,
actionable signal if server readiness genuinely fails to recover under load.

## 2026-08-21: Runtime-wide ceilings use one embedded configuration asset

Context: the production direct-participant creation ceiling was a literal in request enforcement.
The stress harness could therefore offer a workload that was deterministically impossible to stage,
and it learned the limit only after creating hundreds of durable participants and rooms.

Decision: keep non-experiment, process-wide ceilings in
`rust-server/config/runtime-limits.json`, deserialize and validate that asset through the public
`bundled_runtime_limits` API, and embed it into consumers at build time. Production enforcement and
stress preflight both read that API. Stress preflight accounts for one direct participant per
human-agent session and two per human-human session, and fails before artifacts or child processes
when the requested staging burst exceeds the configured window ceiling.

Tradeoffs: changing a bundled ceiling requires rebuilding the server and operational tools, which is
appropriate for a process safety policy and prevents unrecorded environment overrides. The current
stress profile deliberately rejects an oversized burst rather than stretching staging across rate
windows, because paced intake would test a different workload. Experiment-specific capacity remains
in immutable experiment configuration and OS descriptor availability remains a host preflight; they
do not belong in this asset.

Follow-up risks: future hard-coded process ceilings that can make a workload deterministically
impossible should join this asset and gain corresponding preflight checks. Dynamic limitations such
as free storage and provider behavior must continue to be measured at runtime.

## 2026-08-21: Games and agents are session-local logging capability owners

Context: game and agent subclass code needs to record arbitrary diagnostic text from any method or
helper without adding logging parameters throughout the mechanics API. Agents were already created per
session, but the server shared one game value across every live execution and represented each execution
as a `GameRoom` containing only state. That made safe implicit session attribution impossible.

Decision: retain one reusable `GameFactory<Game = G>` and require it to construct a session-owned `Game`
behavior object for each execution. `LiveSession<G>` owns that `G`, its authoritative `G::State`, and a
session-scoped `SessionLogger`; `AgentContext` supplies a separately attributed clone to each agent.
Logger calls synchronously enqueue UTF-8 strings and never perform database I/O. Live sinks accept at
most 64 KiB per entry and 8 MiB per session through a 256-entry bounded queue, then persist
`log` session events asynchronously. Runtime-owned scopes assign `game` or agent participant
and role attribution. Remote agents buffer strings in the Python SDK and return them on every gRPC
response for the Rust runtime to drain. Durable session creation now uses `initializing`, constructs
session objects only after a database identity and logger exist, then advances to `waiting`; failures
record `session_initialization_failed` and become `abandoned`.

The participant-facing vocabulary is now session-only: `/api/sessions`, `public_session_id`, and
`JoinedSession.sessionId`. The process-local `GameRoom` and `RoomParticipant` types are replaced by
`LiveSession` and `SessionParticipant`. SQLite and Rust storage records use `public_session_id`; the
manual 0.3-to-0.4 upgrade renames the old column after a verified backup. Game manifests depend on the
published 0.4 version while the repository Cargo configuration patches that dependency to the local
crate during development.

Tradeoffs: one lightweight behavior value is allocated per session, but compiled code and large
immutable resources remain shared normally through types or `Arc`. Logging is observational: queue
acceptance does not mean durable flush, a crash can lose the tail, and game behavior must not branch on
logging success. Logs can survive a rejected action or later transition-write failure. Arbitrary strings
can contain sensitive material, so extension authors retain responsibility for log content and quotas
bound accidental resource use rather than making it safe.

Follow-up risks: the designed headless `SessionRunner` does not yet exist, so its factory construction
and logger cleanup must be implemented with that runner. Revisit limits and add explicit
dropped-entry/drain telemetry only if production evidence requires it.

## 2026-08-21: Version 0.4 uses distinct game factories and a session-only schema

Context: the first session-logging implementation made `Game` values double as reusable prototypes,
gave loggers an ordinary no-session default, attached agent identity after construction through an
atomic sentinel, and retained the physical `room_id` database column. Session creation also held the
global live-state write lock across database and agent initialization. These shortcuts obscured
lifecycle invariants and introduced an inconsistent error path.

Decision: `Game` now describes only session-owned behavior. A distinct `GameFactory<Game = G>` creates it and
owns configuration validation; `Server<G, F>` receives that factory. Runtime loggers have no production
default or discarding constructor. Agent participants and their durable IDs exist before agent
construction. A session owns a `SessionLogWriter` that closes its queue, drains accepted entries, and is
shut down after terminal game and agent callbacks. Session admission has its own serialization lock,
while the global `MemoryState` lock is held only for short reads and mutations. Initialization errors
remove any installed live session. Version 0.4 deliberately renames the SQLite column to
`public_session_id` and provides no automatic database migration; operators back up the stopped
database and issue one `ALTER TABLE` command. Public route and client renames are intentional 0.4
breaks documented in `docs/migrating-0.3.0-to-0.4.0.md`.

Tradeoffs: game packages add a small explicit factory type and tests must construct real session games.
Admission is serialized during initialization to preserve exact capacity without holding the live-state
lock; slow construction delays new admissions but does not block actions, observations, connections, or
other live-state work. The manual schema step is operationally stricter but keeps runtime schema code
clean and makes rollback a database-file restore.

Follow-up risks: all deployed 0.3 databases must be upgraded while stopped. Remote agents and custom
participant clients must upgrade in lockstep with the 0.4 server. Logger shutdown remains bounded by
the persistence worker and should gain explicit failure/drop counters if operational evidence requires
them.

## 2026-08-21: Arbitrary session text uses the ordinary `log` event type

Context: the first implementation persisted game- and agent-authored strings as `extension_log` events.
The name exposed an internal distinction, and the dashboard's explicit event allowlist initially hid
those rows even though they were stored correctly.

Decision: persist and expose these entries as `log` events. The dashboard treats `log` as an ordinary,
non-housekeeping timeline event and labels it `Game log` or `Agent log` from runtime-owned source
attribution. The corpus export includes the same ordered text as `kind=log`, with nullable actor fields
for game logs and pseudonymous participant/role attribution for agent logs. Existing development
database rows are renamed destructively in place; no compatibility alias for `extension_log` remains.

Tradeoffs: consumers inspecting raw event types must use `log`. The simpler name is stable and leaves
the payload's `source` field responsible for distinguishing game and agent authorship. Arbitrary log
text is research content and therefore makes the corpus's existing content-review requirement more
important; excluding it would make exported session traces incomplete.

## 2026-08-21: RootBot logs decisions without depending on logging success

Context: Great Tree mechanics emitted session logs, but its scripted RootBot exposed no account of
agent construction, observations, or policy decisions in the same session timeline.

Decision: retain RootBot's session-attributed logger and record construction, lifecycle input, queued
actions and canned replies, and response shape. Logging errors are deliberately ignored so queue
pressure cannot change game behavior. Increment RootBot's agent identity version from 1 to 2 because
the research-visible output of the implementation changed even though its playing policy did not.

Tradeoffs: received chat text is duplicated into the session log and therefore consumes additional
storage and carries the same privacy obligations as the already durable conversation event.

## 2026-08-21: Each headless plan executes once with session-local objects

Context: the implemented 0.4 runtime creates each `Game` and automated `Agent` for a specific session.
The earlier agent-agent design placed constructed agents outside `SessionRunner` and also imported an
attempt-and-retry model from batch-job systems that issue #22 does not require.

Decision: `SessionRunner<G>` retains a reusable `GameFactory<Game = G>` but creates one game and two
agents inside each invocation. One deterministic `SessionPlan` is executed exactly once and produces
one finalized `SessionResult`, including when construction or execution fails. There are no attempt IDs
and no automatic session retries. Resume skips every session with a finalized result, successful or
failed; deliberate re-execution requires a new experiment run identity. Callback-level transport
recovery, if a remote-agent adapter supports it, remains inside one logical callback and must be
idempotent. The runner supplies the existing scoped `SessionLogger` during construction and drains it
during cleanup, but logging is not a separate runner abstraction.

Tradeoffs: failed sessions are not silently retried, so transient failures remain visible in evaluation
results and operators must deliberately start another run to gather a replacement sample. In return,
run and plan identity, artifact ownership, resume, and statistical interpretation remain simple: one plan,
one execution, one result.

Follow-up risks: tests must cover constructor failure, cleanup on every exit path, resume over both
successful and failed results, concurrent session isolation, and idempotent remote callback recovery.

## 2026-08-21: Agent YAML selects factories without classifying agent kinds

Context: an earlier experiment schema required `kind: factory` for ordinary agents and `kind: rl` for
learners. Every session participant is created through an `AgentFactory`, including a checkpoint-pinned
learning policy, so this distinction exposed training internals in the seat configuration and duplicated
information already known by the compiled registry.

Decision: every named YAML agent selects its implementation with one `factory` key. The same definition
may include implementation-specific checkpoint and settings fields. When `training.learner` names an
agent, validation requires the registered factory ID to also have an `RLAgent` training capability.
Session planning always resolves the definition to an ordinary checkpoint-pinned `AgentFactory`; only
`ExperimentRunner` asks for the additional learning capability between groups of sessions.

Tradeoffs: the YAML has one uniform agent shape and evaluation code does not branch on an agent kind.
The compiled registry must associate an optional learner implementation with the same stable factory ID,
and an invalid training reference is discovered during experiment validation rather than YAML parsing.

## 2026-08-21: Headless results use run and plan IDs, not dashboard session IDs

Context: the runner design called its deterministic plan hash `session_id`, while the dashboard and
corpus already use `session_id` for a random three-word label attached to a persisted live dialogue.
The two identifiers have different scopes and properties.

Decision: `run_id` identifies one execution of a complete YAML experiment and is reused when that run
resumes. `plan_id` is the deterministic hash of one expanded behavior-changing plan, including its
scenario, repetition, and mirrored-role coordinates; it excludes `run_id` so equivalent plans can be
recognized across runs. Results and resume use the pair `(run_id, plan_id)`. Headless execution does not
create a dashboard dialogue label because it does not create a live database session.

Tradeoffs: result consumers must carry a two-part key rather than an overloaded `session_id`. The names
make scope explicit, permit deliberate re-execution under a new run ID, and avoid suggesting that a
headless artifact can be joined directly to a dashboard or corpus session.

## 2026-08-21: The first headless runner finalizes one result file per plan

Context: issue #22 needs a usable evaluation runner before its optional RL and distributed-artifact
phases. The existing crate is generic over a compiled game, while concrete game binaries own the set of
registered agent factories.

Decision: add a generic public `ExperimentRunner<G>` to the Parlando crate and a Great Tree CLI which
registers its compiled factories before reading strict versioned YAML. The runner expands canonical
plans, executes each through one session-owning task, and atomically writes one JSON result per
`plan_id` under a run manifest. Resume accepts only results whose `(run_id, plan_id)` matches. The
headless logger preserves the existing bounded capability but discards accepted text; structured traces
are the runner's diagnostic artifact. Headless YAML maps semantic secret keys to environment-variable
names while plans and artifacts retain only the existing `game.<key>` references. `fail_fast` forces concurrency to one so stopping cannot cancel
an in-flight session before agent shutdown. Optional learning reuses the same driver: an `RLAgent`
resolves opaque checkpoints to ordinary factories and receives role-safe trajectories between epochs,
while a separate game-specific `RewardFunction` computes numeric rewards from authoritative transitions.

Tradeoffs: one file per plan is simpler and safer for initial resume than shared shards, but very large
runs may eventually need size-bounded worker shards. Sequential fail-fast sacrifices throughput only
for runs which explicitly request immediate stopping. Discarding headless extension logs avoids making
logging a runner subsystem; a later retained backend can replace the sink without changing game or
agent APIs.

Follow-up risks: add provider-specific concurrency/rate limiting before large remote-model runs and
define a concrete Python/gRPC learner transport only when its deployment and payload requirements are
known; it can implement the public `RLAgent` contract without changing session execution.

## 2026-08-21: RL training is a resumable phase over ordinary headless sessions

Context: issue #22 also requires repeated sweeps over training scenarios, learner updates, opaque
checkpoints, and held-out validation. Encoding observations in a game-independent adapter would either
leak game semantics into the runner or duplicate work already owned by the learner. Adding rewards to
the base `Game` trait would burden interactive-only games and prevent multiple objectives.

Decision: register `RLAgent<G>` capabilities by the same factory ID used in uniform agent YAML. A
learner resolves its YAML checkpoint reference, exposes that checkpoint as a normal `AgentFactory<G>`,
owns observation/action encoding and checkpoint persistence, and performs idempotent updates using a
stable run-scoped update ID. Register rewards separately as `RewardFunction<G>` values; they alone may
inspect pre- and post-action authoritative state, and only per-role numbers enter trajectories. The v1
loop captures action-only decisions by the named learner, runs one deterministic scenario sweep per
epoch, updates every configured number of epochs, and validates every configured number of checkpoints.
Every successful update is followed by an atomic checkpoint record. Resume loads finalized trajectory
data and checkpoint records, skips all finalized sessions, and never repeats a recorded learner update.
A failed training session stops checkpoint advancement rather than training on an incomplete epoch.

Tradeoffs: serializing typed observations and actions as JSON keeps the runner and remote boundary
game-independent while leaving interpretation to `RLAgent<G>`. Action-only capture postpones ambiguous
credit assignment for messages and yields. Atomic records make ordinary crashes resumable; the learner
must still treat update IDs idempotently for the narrow crash window after a remote update succeeds but
before its record is finalized. Physical checkpoint storage and save cadence remain learner concerns.

Follow-up risks: a future communication-learning experiment needs explicit environment-step and reward
semantics before expanding trajectories. Large training runs may replace per-result JSON with immutable
shards without changing the public learner, reward, or session contracts.

## 2026-08-21: The first concrete RL experiment uses Python TorchRL and a small Qwen policy

Context: the generic RL runner needs a concrete experiment which can distinguish transport, trajectory,
checkpoint, validation, and resume failures from failures to solve a difficult dialogue game. Rust has
general training frameworks and emerging RL support, but the available path to on-policy PPO over a
LoRA-adapted pretrained Qwen model would require building more model and algorithm integration than the
Parlando infrastructure being tested.

Decision: implement `experiments/cue-choice-rl` as a two-action contextual-bandit session. A scripted
player-B dealer acts first; a player-A learner then maps one of four private nonsense cues to one of four
typed choices. Use Python TorchRL 0.13.3 for generalized advantage estimation with an explicit clipped
PPO loss, Transformers and PEFT LoRA,
and the Apache-2.0 `Qwen/Qwen2.5-0.5B-Instruct` checkpoint. Keep game mechanics, scheduling, reward, and
experiment coordination in Rust. Host ordinary checkpoint-pinned inference through the existing agent
service and add a narrow learner gRPC service for idempotent training batches; synchronous checkpoint
resolution validates the opaque YAML string locally, and Python interprets that ID when loading policy state.
Use a fixed categorical action head and scalar value head over the Qwen representation rather than
free-form JSON generation. Training and validation share the cue mapping but use disjoint seed-derived
nonces. Treat learning convergence as a diagnostic; infrastructure acceptance depends on complete,
role-safe trajectories, checkpoint provenance, validation cadence, and replay-free resume.

Tradeoffs: the one-step learner transition makes reward and return calculation inspectable and the
fixed action head avoids conflating RL with output parsing. It does not test long-horizon credit
assignment, communication learning, or open-ended language generation. Python adds a local service and
environment to operate, but preserves the intended ownership boundary: the learner encodes
observations, trains the model, and resolves checkpoint storage while Rust owns authoritative state.

Implementation detail: extend `RLAgent::train` with a runner-owned `RLTrainingContext`, and send its
normalized settings in the learner request rather than configuring training as a side effect of agent
construction. Add a generic Python SDK helper for registering `AgentService` on an existing asynchronous
gRPC server, allowing this experiment's inference and learner services to share one loopback endpoint.
Preload Qwen once per process and keep session agents lightweight. Default to Apple's MPS backend on Mac,
with explicit `cpu` and capability-selecting `auto` alternatives; keep all training tensors in float32 for
MPS compatibility. Initialize the categorical policy and scalar value heads to zero so the first policy is
uniform and the infrastructure experiment does not depend on an arbitrary randomly favored action.
Include a dependency-light random backend so protocol, checkpoint, validation, and
resume behavior can be tested without downloading PyTorch or a model.

Follow-up risks: validate TorchRL, Transformers, PEFT, and PyTorch versions together before pinning the
Python lockfile. The learner keeps intermediate checkpoints in memory, which makes resume
depend on the same Python process; a production learner must durably retain every checkpoint referenced
by an active run. Reuse the existing remote-agent endpoint and credential checks before allowing the
learner service off loopback. A production learner must also reload learner-owned disk checkpoints;
version 1 only writes selected snapshots and assumes one learner process remains alive for the run.

## 2026-08-21: Remote agent transport is game-independent and owns RL transport

Context: requiring each experiment to implement `RemoteGrpcAgent<G>` and a second Rust learner adapter
duplicated serialization, checkpoint routing, authentication, and protobuf code. The transport never
interprets a game's observation or action; Rust's type parameter is needed only while calling the typed
traits, before values cross the process boundary as JSON.

Decision: expose one non-generic `agent::grpc::RemoteAgent`. Blanket implementations of
`AgentFactory<G>`, `Agent<G>`, and `RLAgent<G>` perform typed serialization internally without storing
`G`. Its YAML has transport-owned fields plus one arbitrary `config` value delivered unchanged to the
remote process. A checkpoint is a separate optional `CreateAgent` field, and training uses the generic
Parlando learner protocol. The Python SDK registers both services and converts their envelopes to plain
Python dictionaries. Experiment packages implement only game behavior, reward logic, model inference,
and training.

Tradeoffs: arbitrary remote configuration cannot receive dashboard controls derived from a compiled
schema, but it avoids coupling Parlando releases to model-specific parameters. JSON remains the common
representation, so game values must be serializable and remote agents remain responsible for validating
their own configuration and decoding their own observations. Transport credentials remain explicit
Rust-owned fields and are never mixed into the opaque configuration.

## 2026-08-22: Automatic learner device selection prefers CUDA

Context: the cue-choice learner originally treated `auto` as an Apple-specific choice between MPS and
CPU. The same experiment should use an available NVIDIA accelerator without requiring a separate code
path or a manually pinned process argument.

Decision: make `auto` select CUDA first, Apple MPS second, and CPU otherwise. Accept `cuda`, `mps`, and
`cpu` as explicit process choices and fail at startup when an explicitly requested accelerator is not
available. Keep the experiment YAML on `auto` so it remains portable across CUDA hosts and Macs.

Tradeoffs: CUDA now takes precedence on machines exposing both CUDA and MPS-compatible interfaces,
which is the intended accelerator preference. Model weights and training tensors remain float32 for
cross-device consistency rather than selecting device-specific mixed precision automatically.

## 2026-08-21: Python agent SDK and protocol are top-level components

Context: the independently packaged Python agent SDK lived below `rust-server/python`, while its gRPC
contracts were duplicated between the Rust crate and Python package. This layout implied Rust ownership
of a language-neutral boundary and required a byte-for-byte drift test between protocol copies.

Decision: make `parlando-agent-sdk` a top-level sibling of `rust-server` and `js-client`, and keep the
authoritative remote-agent and reinforcement-learning contracts in the top-level `proto` directory.
Rust's build script and the Python binding generator both consume those shared definitions. Published
Python artifacts contain generated bindings, while protocol regeneration remains a repository
development operation.

Tradeoffs: an SDK source checkout detached from the repository cannot regenerate its bindings, but
normal installation and use do not require protoc. A shared protocol directory makes ownership and
cross-language review explicit and eliminates duplicated source contracts.

## 2026-08-21: Cross-language audio contracts enter the running lifecycle

Context: microphone frames are relayed while a session is waiting, but transcription intentionally
starts only after both participants have connected to the game transport and the session becomes
running. The JavaScript mute integration fixture opened audio transports without game transports, so
it could verify relay but could never observe a transcribed frame.

Decision: make the integration fixture connect both game WebSockets and wait for both targeted
`session_started` messages before testing live, muted, and resumed capture. Keep the production
running-session transcription gate unchanged.

Tradeoffs: the test performs more lifecycle setup, but now exercises the same transport ordering as a
real participant and preserves the privacy boundary that waiting-session audio is not transcribed.

## 2026-08-21: The post-0.3 reliability round starts at ownership and failure boundaries

Context: Parlando already had broad happy-path coverage after 0.3, while 0.4 added session-scoped game
construction, bounded asynchronous session logging, semantic agent configuration, resumable headless
experiments, and additional browser reconnect behavior. Raising aggregate line coverage alone would not
protect the concurrency and isolation contracts that make these components safe to refactor.

Decision: add the first post-0.3 tests at deterministic ownership and failure boundaries. Rust tests
exercise exact UTF-8 and aggregate logging limits, concurrent admission, queue refund behavior, semantic
agent-schema/value validation, secret failure redaction, opaque checkpoint identity, canonical hashing,
fixed-role rewards, and schedule handoffs. Browser tests exercise heartbeat ownership across replacement
sockets, reconnect backoff, credential retention, and best-effort leave/diagnostic failures. Treat
participant-facing teardown and diagnostics as non-throwing operations because transport closure and
diagnostic serialization are auxiliary to the authoritative server lifecycle.

Tradeoffs: this batch favors fast deterministic component tests over a larger browser or process-level
suite. It does not yet supply a fault-injecting database, shared Rust/TypeScript protocol fixtures, live
session-log persistence tests, or the full agent-runner timeout and crash-recovery matrix. Those remain the
next enabling layer because they require explicit controllable seams rather than timing-based tests.

Follow-up risks: direct construction of the logger's private queue state intentionally tests its resource
accounting primitive; live SQLite attribution and shutdown draining still need an integration fixture.
Reconnect timing uses fake browser clocks and should be complemented by one packed-client/real-router test.

## 2026-08-21: Terminality and protocol compatibility are enforced below the UI runtime

Context: the second reliability batch exercised terminal operations concurrently and reused protocol
messages across Rust and TypeScript. Runtime locks normally serialize completion, abandonment, and expiry,
but the SQLite transition method could still append a transition and overwrite an already terminal row if
called after another terminal writer. Separately, handwritten Rust and TypeScript examples could agree
with themselves while drifting from one another.

Decision at the time: make `commit_session_transition` inspect terminal status inside its transaction and
no-op after `completed`, `expired`, or `abandoned`, matching the then-current expiry and abandonment
behavior. Add a three-way durable terminal race and a live logger drain/attribution integration test. The
participant protocol fixture introduced by this decision has since been replaced outright by protocol 2;
there is no retained protocol-1 fixture or compatibility decoder. Add an external-consumer TypeScript
compile test and an npm file-allowlist test to the normal JavaScript test lane.

Tradeoffs: terminal transition attempts after terminality return success as idempotent no-ops, so callers do
not learn which concurrent terminal operation won without reading the session. The JSON fixture is a
contract corpus rather than a generated schema; adding a protocol variant requires reviewing one shared
file and both consumers. Package verification invokes `npm pack --dry-run`, which costs an extra build but
tests the actual publication allowlist.

Follow-up risks: storage fault injection between individual action-transaction statements still requires a
dedicated controllable store seam. The shared corpus covers protocol shape, while a packed-client browser
against a real router remains responsible for behavioral compatibility.

## 2026-08-21: Active errors and reconnect expiry are participant-visible exact contracts

Context: rendered state-machine tests showed that action-rejection and server errors were stored while a
game was active but not rendered anywhere, because the active branch returned only the game component.
They also showed that exponential backoff could schedule the next expiry check after the documented
five-minute reconnection window.

Decision: render the standard online-error element beside an active game and cap every reconnect delay by
the exact time remaining in the five-minute window. Keep reconnect ownership generation-scoped, preserve
the existing 1/2/5/10-second progression, and stop scheduling once the deadline is reached.

Tradeoffs: the SDK now renders a small error sibling outside the game renderer, giving standard clients a
consistent failure channel without extending the public `GameSession` API. A retry can occur exactly at
the deadline before its failure marks the window expired; no attempt is scheduled beyond it.
## Shared audio wire fixtures and defensive browser teardown (2026-08-21)

- Context: PCM framing is implemented independently in Rust and TypeScript, while browser media resources can throw during both setup and teardown after device loss.
- Choice: keep representative version/sequence/timestamp headers in `proto/pcm_frame_v1.fixtures.json` and consume them from both suites. Treat the microphone level analyser as optional, and make teardown best-effort so one faulty browser resource cannot leak the rest.
- Tradeoffs: the fixture intentionally covers framing metadata rather than duplicating a 960-byte zero payload. Browser analyser failure no longer prevents otherwise valid microphone capture, so UI level metering may be absent while voice remains usable.
- Follow-up risk: any future audio protocol version must update the shared fixture and both encoders together.

## 2026-08-22: The storage trait remains an acceptable single-implementation seam

Context: `ExperimentStore` currently has one production implementation,
`SqliteExperimentStore`. A code audit questioned whether the broad trait added useful abstraction
while no alternative backend exists.

Decision: retain `ExperimentStore` and its shared trait-object handle. The boundary is useful for
keeping authentication, session logging, routing, and runtime orchestration independent of SQLx
types, and a single implementation is acceptable. Do not invent another backend or split the trait
merely to justify it; revisit the shape only when a concrete second implementation or a narrower
fault-injection seam requires a change.

Tradeoffs: the trait remains broader than any individual consumer and adds dynamic dispatch, but
the cost is small beside database I/O and preserves a clear durable-storage ownership boundary.
SQLite-specific behavior may be documented by methods whose current implementation necessarily
reflects the only supported backend.

Follow-up risk: new methods can turn the trait into an unstructured mirror of the SQLite store.
Add operations only when they express an application-level atomicity or query contract, not as a
mechanical wrapper around arbitrary SQL.

## 2026-08-22: Browser games consume server-owned observations and affordances exactly

Context: Space Game retained a TypeScript copy of its initial state, transitions, derived systems,
and available-action calculation. The live client could use that calculation when the server sent
an empty action list, creating a second mechanics authority and preserving unused action variants.

Decision: keep all mechanics, derived semantic state, and action availability in Rust. The browser
models the exact role-specific observation, renders its supplied `systems` value, and submits only
actions advertised by the server or explicit movement controls that the server validates. Remove
the duplicate TypeScript engine and action variants that no current affordance produces.

Tradeoffs: presentation code still understands action shapes to label and preview controls, and
movement keys may optimistically submit a rejected move. It no longer predicts authoritative state
or manufactures a supposedly available device action.

Follow-up risk: changes to a game's observation or action JSON remain a coordinated Rust and
TypeScript contract change until game-specific type generation is introduced.

## 2026-08-22: Protocol inputs and agent configuration fail closed

Context: the browser accepted parsed JSON as a trusted server union, while agent startup repaired
missing factories and secret references from registration order and naming conventions. Those
fallbacks hid configuration drift and made behavior depend on incidental catalogue order.

Decision: decode every participant-server message at runtime, reject unknown versions, variants,
and malformed fields, and make handling exhaustive. Require every configured agent to name its
factory. Publish server-owned secret references in the agent field descriptors and require the
dashboard to use those references exactly; do not repair stored configurations during startup.

Tradeoffs: malformed messages and incomplete old agent configurations now fail visibly instead of
being guessed into a runnable shape. Adding a protocol variant requires updating the decoder and
shared fixtures, and adding a secret field requires declaring its reference in the server schema.

## 2026-08-22: Consent and conversation records preserve their direct source

Context: consent text was expanded from placeholder-bearing templates at read time, so the hash
could describe inferred text rather than the stored revision. Typed chat, voice transcripts, and
agent messages also followed separate persistence and broadcast paths with pseudo-attribution in
metadata.

Decision: reject consent placeholders and hash the exact stored consent items. Require concrete
study information in each configured revision. Commit all conversation messages through one
origin-aware pipeline that derives the sender role from the participant identity, persists the
same canonical event, broadcasts it, updates activity, and notifies agents.

Tradeoffs: operators must save complete participant-facing text rather than rely on installation
fallbacks. The shared message path is stricter about sender identity and metadata size, but removes
three subtly different definitions of an accepted conversation message.

## 2026-08-22: Generated Python protocol modules use normal package imports

Context: the Python SDK modified `sys.path` and dynamically imported checked-in generated modules,
and its protobuf presence helper supported arbitrary lightweight objects in production code.

Decision: use package-relative imports for generated modules, make proto generation rewrite gRPC's
absolute sibling imports to relative imports, and rely on protobuf `HasField` semantics for optional
checkpoint identity. Keep test doubles conformant to the production request contract.

Tradeoffs: generated modules must remain present in the package, as already required by the build.
Regeneration includes a small deterministic import-normalization pass, avoiding runtime import-path
mutation and test-only compatibility branches.

## 2026-08-22: SQLite starts from one current schema baseline

Context: application startup carried eleven historical, data-rewriting SQLite migrations even
though this codebase no longer promises in-place upgrades from those releases. The ladder also
kept obsolete columns, status names, identifier schemes, and provider-placement rules alive in
production and in tests.

Decision: create the complete current schema directly and stamp empty databases with baseline
version 11. Open databases already stamped at version 11 without mutation. Reject older versions
and populated unstamped databases with an explicit export/import instruction; do not guess at or
rewrite their historical shape in the running server.

Tradeoffs: deployment upgrades from a pre-baseline database require using a compatible older
release to export data and importing it into a fresh current database. New installations have a
shorter, deterministic initialization path, and production no longer contains dormant legacy data
transformations.

Follow-up risk: the schema version must be advanced deliberately whenever an in-place current-era
schema change is introduced. That change should come with a bounded upgrade policy rather than a
new indefinite historical ladder.

## 2026-08-23: Session endings derive participant-specific recruitment outcomes

Context: game completion, explicit departure, lifecycle expiry, transient disconnects, page reloads,
and Prolific completion paths affect a shared session and its two participants differently. Treating
each case as one session status cannot express that a departing participant withdrew while their
partner lost a peer, and a long reconnect grace leaves the connected participant without useful
feedback.

Decision: design one atomic session-ending record that derives one immutable outcome per human
participant. Detect dead game connections after approximately five seconds, pause play, and reserve
the role for a fifteen-second reconnect window displayed through a reusable client widget. Support
same-tab reload with an opaque credential in experiment-scoped `sessionStorage` and an authenticated
resume snapshot. Keep all Prolific participant and submission identifiers in a deletable,
dashboard-only table excluded from every export; map provider-neutral participant outcomes to
dashboard-configured completion paths. The complete design and deterministic lifecycle test matrix
are in `docs/session-ending-prolific-reconnection-design.md`.

Tradeoffs: a short reconnect window handles ordinary transport interruptions and reloads without
making the partner wait minutes, but it does not recover a session after a server restart or across
devices. The runtime and client gain an explicit pause/resume protocol, while generated games only
render and style the supplied widget. Persistent Prolific correlation remains present in database
backups until their separate retention window expires.

Follow-up risks: finalize the game outcome-key API, Prolific repeat-submission and retention policy,
experiment-clone behavior, and the browser automation lane before implementation.
## 2026-08-23: Centralize participant termination, reload recovery, and recruitment handoff

- Context: Games need consistent behavior for normal completion, participant departure, short network loss, matchmaking timeout, and infrastructure failure. Prolific correlation must remain private and excluded from research exports.
- Decision: The Rust runtime owns durable shared termination reasons and recipient-specific participant outcomes. Prolific URL identifiers live only in `prolific_submissions`; the table is deliberately absent from export queries. Experiment configuration maps provider-neutral outcomes to completion codes. The JS participant SDK owns tab-scoped credential recovery, a 15-second reconnect loop, the stylable partner countdown, the standard terminal surface, and the provider handoff. Game code supplies only normal `Game::completion` data and gates controls on `interactionEnabled`.
- Tradeoffs: `sessionStorage` makes ordinary reloads recoverable without sharing credentials across tabs, but cannot recover a closed tab or browser. A 15-second grace period admits quick Wi-Fi/page-reload recovery while avoiding a long ambiguous wait for the partner. Prolific identifiers remain persistent until the private table is explicitly retired.
- Risks and follow-up: Server restart recovery still requires reconstructing live game state from durable events and is outside this change. The runtime should eventually expose an explicit read-only resume endpoint so reload recovery does not reuse the join operation. Add operational retention/deletion controls for `prolific_submissions` before retiring Prolific integration.

## 2026-08-23: Configuration validity never gates historical session inspection

- Context: Tightening a configuration validator made an older experiment revision temporarily invalid. The dashboard coupled configuration loading to workspace visibility, hiding sessions that remained intact in storage.
- Decision: A selected experiment always retains its workspace tabs and read-only session access. Configuration load failure disables only configuration editing and lifecycle actions that require a runnable configuration; it does not hide sessions, notes, exports, or privacy inspection.
- Tradeoff: Some non-configuration views may independently report that a runtime-specific operation is unavailable, but durable historical data remains inspectable.

## 2026-08-23: Generated Prolific codes use two dictionary words

- Context: Numeric suffixes made generated completion codes harder to read and communicate.
- Decision: The dashboard generator concatenates one uppercase adjective and one uppercase noun, without digits or punctuation. Manual Prolific codes remain accepted as alphanumeric strings because externally created codes may not follow this presentation convention.
- Tradeoff: The bundled 32-by-32 vocabulary has 1,024 combinations, so experimenters should still check that codes are distinct within an experiment.

## 2026-08-23: Dashboard participant links simulate Prolific intake (superseded)

- Context: Prolific-enabled intake correctly rejects participant pages without all three provider query parameters, which made the dashboard's plain participant link unusable for local testing.
- Historical decision: For a Prolific-enabled experiment, the dashboard link included a synthetic `PROLIFIC_PID`, the experiment's exact configured `STUDY_ID`, and a synthetic `SESSION_ID`. Synthetic participant and session identifiers were unique per dashboard page load and stable across its periodic rerenders. Direct experiments retained their clean participant URL.
- Superseded on 2026-08-27: The participant URL is now selected when an intake run starts and retained until that run ends. Reloading the dashboard therefore keeps the same synthetic identity for Local Preview, while provider-backed runs keep their literal Prolific placeholders.

## 2026-08-23: Use Prolific completion-path terminology in configuration

- Context: Outcome-specific code fields were technically correct but did not tell experimenters where the values correspond in Prolific.
- Decision: Dashboard copy calls each mapping a Prolific completion path with a custom completion code, points to Data collection → Completion paths, and explains that Parlando constructs the completion URL from that code. Parlando outcome names remain visible so the routing condition is unambiguous.
- Tradeoff: Submission-processing actions remain configured in Prolific rather than duplicated in Parlando; the dashboard does not prescribe approval, return, or rejection policy.

## 2026-08-23: Declining consent returns to Prolific before registration

- Context: Prolific participants must be able to decline configured consent items, while direct participants do not need a provider-return action.
- Decision: When consent items exist, the shared participant landing page places a succinct “Do not consent” secondary action beside “Enter waiting room” only for Prolific intake. It returns to the participant's Prolific submissions page before participant registration, consent recording, or matchmaking; Parlando does not require or emit a no-consent completion code.
- Tradeoff: Generated games must style the shared `.parlando-decline-consent` lifecycle class. The participant completes Prolific's own “Stop Without Completing” flow rather than following an outcome-specific completion-code redirect.
- Superseded on 2026-08-31: Prolific experiments now require a dedicated `no_consent` completion path for provider-backed activation. Active Local Preview and provider intake both redirect through that configured completion code; incomplete local drafts omit the decline link rather than emitting an empty code.

## 2026-08-23: Dashboard configuration reads tolerate invalid revisions

- Context: A newly introduced validation rule made an older stored revision unavailable in the dashboard, preventing the experimenter from viewing and correcting an otherwise structurally readable configuration.
- Decision: Dashboard configuration reads deserialize and normalize stored revisions without applying current runtime validation. Validation failures are returned as activation issues inside the editable configuration view. Saving, activating, and constructing a runtime remain strictly validated.
- Tradeoff: A structurally malformed configuration can still lack fields the curated editor expects, but the raw revision remains visible and repairable instead of turning the entire configuration tab into an error page.

## 2026-08-24: Schema upgrades are backup-first and in place

- Context: Rejecting an older database or requiring export and import preserved a clean runtime but made ordinary code upgrades operationally disruptive. A long-lived migration ladder would create the opposite problem by retaining obsolete historical transformations indefinitely.
- Decision: Before a schema change, make a restorable backup of the database and then apply one explicit, bounded transformation to the database in place. Do not introduce a general migration framework or an accumulating migration ladder. This decision supersedes the 2026-08-22 export/import requirement for post-baseline schema changes.
- Tradeoff: Each schema change needs a deliberately reviewed upgrade operation and a verified backup rather than relying on generic automatic migration machinery. Rollback restores the backup.

## 2026-08-24: Historical experiments remain inspectable and repairable

- Context: Configuration and schema evolution can make an older experiment invalid according to current runtime rules. Treating that invalidity as a read failure hides the information needed to diagnose and repair the experiment.
- Decision: Every code and schema change must preserve dashboard display of stored experiments, their sessions, and an editable configuration. Current validation may classify an older experiment as invalid and block new sessions, but it must be reported inside the dashboard rather than gating historical reads or editing. Compatibility tests must cover session visibility and configuration display/editing for an older stored experiment.
- Tradeoff: Dashboard read models must tolerate some values that runtime construction and saves reject. The system maintains a strict write and activation boundary while deliberately accepting a broader read boundary.

## 2026-08-24: Games supply completion content inside a provider-neutral terminal shell

- Context: A generic “Session complete” screen hid the meaning of Great Tree's cooperative win, while letting games replace the entire terminal surface would duplicate Prolific handoff behavior and invite provider-specific branches.
- Decision: `ParticipantApp` accepts a game-owned `renderCompletion` function for normal game completion. The game controls the semantic result content and styles the shared terminal classes. Parlando appends its premade Prolific handoff widget only when the participant arrived through Prolific; direct and Prolific participants otherwise see identical game-ending content. Runtime failures and disconnect outcomes remain runtime-owned.
- Tradeoff: Generated game CSS must deliberately cover the terminal shell and handoff classes. Games gain presentation control over normal completion without gaining recruitment-provider logic.

## 2026-08-24: Recruitment source is a dashboard-only session fact

- Context: The session summary labeled a legacy, hardcoded `mode` value as “Mode,” causing Prolific sessions to appear as Direct. Prolific correlation is intentionally stored only in a private dashboard table and excluded from exports.
- Decision: Replace the dashboard's Mode row with Recruitment. Derive its value from the selected session's private participant rows: any attached Prolific submission makes the session Prolific; otherwise it is Direct. Do not repurpose the exported session `mode` field to carry recruitment-provider information.
- Tradeoff: Recruitment is derived when the dashboard loads participant details rather than duplicated on the exported session record. This also classifies existing sessions correctly without a database schema change.

## 2026-08-24: Shared lifecycle cards remain content-sized inside game shells

- Context: Great Tree's full-height flex shell stretched the shared landing card along the flex cross axis. Because the card uses a grid layout, its rows then distributed the extra viewport height between the consent, voice-preparation, and action sections, producing large empty gaps and an apparently broken landing page.
- Decision: Game shells that host Parlando lifecycle surfaces explicitly align their cards at the start of the flex cross axis. Lifecycle cards size to their content with a bounded responsive width; terminal cards opt into vertical centering with their own margins. Great Tree styles every shared lifecycle state as one coherent system, including consent, voice preparation, waiting, reconnect, errors, normal completion, and Prolific handoff. The game-generation guidance requires desktop and mobile checks for these states.
- Tradeoff: Games retain control of their visual language and terminal content, while their stylesheet must honor the shared lifecycle class contract. The explicit alignment prevents future full-height shell changes from silently stretching lifecycle grids.

## 2026-08-24: The shared Prolific handoff owns completion-code copying

- Context: Participants may need to transfer a custom completion code from Parlando into Prolific. Selecting and copying codewords manually is needlessly error-prone, while implementing clipboard behavior in each game would duplicate recruitment-provider logic.
- Decision: The JS client's premade Prolific handoff places a keyboard-accessible copy button beside the completion code. It uses the browser Clipboard API, announces successful copying, and preserves a selectable code with explicit manual-copy guidance when clipboard access fails. Games style the shared code row, button, and status classes but do not implement copying themselves.
- Tradeoff: Automatic copying requires a secure browser context and clipboard permission. Failure remains recoverable because the code is visible and selectable, and participants can still continue through the existing Prolific return link.

## 2026-08-24: Recruitment source survives deletion of private Prolific identifiers

- Context: Dashboard session details display private Prolific participant, study, and submission identifiers beside their A/B assignments. Those isolated correlation rows are intended to be purged independently of durable experiment sessions. Inferring recruitment source from the presence of those rows would relabel historical Prolific sessions as Direct after the purge.
- Decision: Recording a Prolific submission transactionally stamps the participant record's non-identifying `identity_provider` classification as `prolific`. Session inspection returns that classification separately from optional data in `prolific_submissions`. The dashboard derives its Recruitment label from the durable classification and renders the detailed Prolific identifier line only while the private row exists. Existing rows created before this rule are corrected once by backing up and editing the database in place. Deleting all Prolific correlation rows for an experiment therefore leaves A/B assignments, research identifiers, outcomes, and the Prolific recruitment classification intact.
- Tradeoff: The durable participant record retains the fact that recruitment used Prolific, but none of Prolific's participant, study, or submission identifiers. This provider classification remains dashboard-visible and does not add those private identifiers to exports.

## 2026-08-24: The dashboard uses three responsive workspace modes

- Context: The experiment catalogue, session list, and session detail remained side by side until a 900-pixel viewport breakpoint. At intermediate widths, the two fixed navigation columns left too little room for session facts, participant assignments, and event text, causing overlapping labels and near-vertical event content.
- Decision: Keep all three columns only on wide desktops. At 1240 pixels and below, move the experiment catalogue into its existing menu drawer while retaining the useful session-list/detail split. At 900 pixels and below, stack session detail beneath a horizontally scrollable session picker. At 760 pixels and below, stack session facts and A/B participant cards. Navigation tabs scroll horizontally rather than widening the document, and long identifiers and event values wrap inside their own bounded columns.
- Tradeoff: Intermediate-width users open the experiment catalogue through the menu button, but gain a substantially wider working area for the selected session. Narrow layouts favor readable vertical flow over simultaneous visibility of every dashboard column.

## 2026-08-24: Explicit leave is a durable terminal handshake

- Context: The participant client sent the runtime's `leave` message and immediately closed its game socket. The runtime durably recorded abandonment and broadcast the correct recipient-specific outcomes, but the leaving browser destroyed the channel carrying its own `withdrew` result and continued to look like an active game.
- Decision: `session.leave` immediately disables reconnection, voice, heartbeats, and game input and replaces the game with an “Ending session” state, while keeping the game socket open until the runtime sends `session_ended`. The shared terminal screen then explains the withdrawal. A Prolific participant receives a premade no-code widget directing them back to Prolific to return the submission; voluntary withdrawal does not invent a completion code. The runtime treats both completed and abandoned sessions as closed to actions and conversation, and abandoning a human-versus-agent session closes the agent inbox and runs its normal shutdown lifecycle.
- Tradeoff: The browser briefly waits for durable server confirmation instead of presenting an optimistic final result. This preserves the authoritative participant outcome and recruitment instructions; a future transport-level acknowledgement or authenticated HTTP fallback may be useful if explicit leave must also survive a connection failure at the exact moment of departure.

## 2026-08-24: Public documentation and internal notes have separate lifecycles

- Context: Versioned `docs/` had accumulated implementation proposals, research working papers, and future-work language beside operator and API guides. This made it unclear which behavior users could rely on and published internal work products as if they were supported contracts.
- Decision: Everything in `docs/` is versioned, user-facing documentation written in the academic-exposition style. It explains supported tasks, current contracts, concrete limitations, migration behavior, and public worked examples. Internal plans, design specifications, audit working papers, private drafts, and decision history live in ignored `notes/` and are never linked from versioned documentation. When an internal design becomes implemented behavior, write fresh public documentation rather than promoting the working note unchanged.
- Tradeoff: Internal reasoning is not available in a normal source checkout, so public documents must be self-contained and code changes must not depend on readers finding a private note. Contributors retain local history in `notes/`, while the repository exposes a smaller and more reliable public manual.

## 2026-08-24: Server and clients share one participant-state inventory

- Context: Participant lifecycle is currently represented by overlapping React booleans, runtime session-status strings, connection/readiness flags, and one-shot WebSocket messages. Keeping the socket open during explicit leave fixes the ordinary early-close bug, but losing the connection after the durable terminal write can still prevent the browser from receiving its recipient-specific outcome and Prolific handoff.
- Decision: Define one canonical `Registered | Waiting | Active | Paused | Ended` participant-state inventory shared by Rust and TypeScript. The server owns the authoritative state and clients replicate authenticated snapshots. Participant states are projections of a clean `Forming | Running | Ended` session lifecycle, running-session interaction availability, membership, and recipient-specific terminal results. Client connection, leave-request, rendering, and audio states remain separate local machines. `Ended` is immutable, explicit leave is allowed from every attached nonterminal state and is recoverable through a read-only participant-state snapshot, and all meaningful input is accepted only in `Active`. Leave disables game interaction but retains terminal reconciliation, superseding the earlier decision to disable reconnection completely while waiting for `session_ended`. The internal design and test matrix are recorded in `notes/participant-state-machine-design.md`.
- Tradeoff: This replaces the existing lifecycle scheme across the participant protocol, storage model, server transition service, JavaScript controller, input guards, external clients, tests, skills, and public documentation. The core session model drops completed, abandoned, expired, and failed as lifecycle values in favor of `Ended` plus a structured cause. Protocol version 2 is a coordinated clean cut: no legacy lifecycle messages, WebSocket leave operation, coarse-status export, or compatibility adapter remains. The design makes terminal delivery recoverable without claiming running-game recovery after a server restart.

- Implementation: Persist `SessionEnd` and every human role's `ParticipantResult` atomically, retain those results after live-room cleanup, and expose them through `GET /api/participant-state`. Use idempotent HTTP leave and keep the socket open until its terminal response arrives. WebSocket protocol 2 begins with and changes lifecycle through complete `participant_state` snapshots; compact game deltas remain non-lifecycle messages. The browser reducer validates the seven-edge graph and tracks connection synchronization separately. SQLite schema 13 is the clean baseline and intentionally rejects earlier populated schema versions rather than carrying old columns or conversion branches in production.

## 2026-08-26: Prolific uses researcher-owned completion paths

Context: The existing Prolific integration separates provider correlation, Parlando authentication,
and provider-neutral participant outcomes. A fresh audit found that role B loses its Prolific runtime source,
duplicate `SESSION_ID` values create split identities, required consent can be bypassed by a
Prolific client, and participant deletion retains private provider identifiers. Current Prolific
guidance also requires return handling for non-consenting participants and compensation for
unmatched dyadic participants.

Decision: Provide one first-class, API-connected Prolific mode. Connect a workspace once, bind one
Prolific study to an immutable experiment revision, verify launch through Secure external URL when
available or submission lookup otherwise, and atomically map one submission to one resumable
Parlando admission. The researcher creates five completion paths in Prolific and copies exactly one
code for each of `completed`, `partner_left`, `partner_unavailable`, `timed_out`, and
`technical_failure` into the experiment. Parlando neither generates codes nor accepts a free-form
game-code map. Enabling Prolific without five valid, distinct codes prevents activation. Both human
roles retain Prolific identity, and consent is a server-enforced admission guard. A fixed
server-owned waiting deadline is shown as a participant countdown. An unmatched participant uses
the `partner_unavailable` path, implemented as a custom path labelled Unmatched in Prolific, whose
action requests a return. This is not a Prolific Screened out path: unmatched pairing is not an
eligibility screen-out, and the screen-out fixed-payment mechanism is not used.
Voluntary waiting-room departure and expiry of the fixed waiting deadline remain different Parlando
facts but use the same Unmatched completion path. Parlando records only waiting start, terminal
time, elapsed duration, factual cause, selected code, and handoff state. It has no compensation
configuration, billable-time rule, monetary amount, payment queue, bonus reconciliation, or
bonus-payment API use. The session list labels both outcomes **Unmatched**, while the detail
distinguishes voluntary departure from deadline expiry and shows the unsuccessful waiting duration.

For a running session that ends asymmetrically, the dashboard labels the session **Partner left**
and shows both role assignments and participant outcomes. It identifies who left, became inactive,
or failed to reconnect; who remained; the relevant timestamps; and both completion handoffs.

A session-wide inactivity safeguard applies only when neither participant produces meaningful
activity during the full configured interval. After a participant-visible warning expires, both
participants map to the `timed_out` return path. If game logic can identify one participant as
blocking progress while the other acts in good faith,
the blocking participant instead receives `participant_inactive` and the partner receives
the approving `partner_left` path. Explicit departure remains `left_game`; both non-completing
outcomes use the same `timed_out` return path. The complete proposed protocol is in
`notes/prolific-integration-design.md`.

Tradeoffs: Requiring a Prolific API connection makes workspace secret management and provider
availability part of the installation, while five copied codes add a small deliberate setup step.
The researcher handles all compensation work in Prolific. This keeps Parlando configuration and UI
small, at the cost of providing no Parlando-side payment reminders or payment-state tracking.
Prolific remains the source of truth for each code and its processing action; Parlando's preflight
detects copying and action errors before activation.

Superseded decisions: This proposal supersedes Parlando-generated code suggestions, automatic
completion-path creation, every Parlando-side compensation field or calculation, waiting-payment
work queues, bonus reconciliation, treating a voluntary waiting-room exit differently from waiting
expiry, and full-reward approval for unmatched submissions.

Follow-up risks: Prolific exposes Secure external URL only to certain workspaces, so the adapter
must discover actual capabilities. Preflight must determine whether the API exposes enough
completion-path action detail to prove that the Unmatched path requests a return. The schema change
requires identifying the live database, making a consistent adjacent backup, converting it
transactionally, and verifying integrity, foreign keys, row counts, provider links, and completion
handoff values before application restart.

## 2026-08-27: Implement the Prolific protocol as one phase-oriented vertical change

Context: The agreed Prolific design spans workspace connection, experiment preflight, authenticated
launch, idempotent admission, matching, phase deadlines, recipient-specific outcomes, participant
handoff, restart behavior, and dashboard inspection. The current code already has server-enforced
consent, a shared participant lifecycle, and atomic terminal storage, but its durable Prolific
identity is created indirectly, role B hardcodes a direct source, timeout outcomes are too coarse,
waiting deadlines are not participant-visible, and the dashboard exposes insufficient factual
detail. Implementing isolated completion-code or waiting-room changes would leave the overall
protocol inconsistent.

Decision: Implement the design in the dependency order recorded in
`notes/prolific-integration-implementation-plan.md`. First prove the exact provider API, Secure
external URL, completion-action, and webhook contracts. Then make one clean backup-first schema
change; add a narrow server-only provider adapter; enforce workspace, study, and five-path
preflight; map one verified Prolific submission atomically to one stable admission; centralize fixed
phase clocks and exhaustive participant-outcome derivation; update the shared Rust/TypeScript
protocol and widgets; restore nonterminal phases after restart; and expose server-derived factual
session detail and limited provider-status reconciliation. Reuse the existing server consent gate
instead of duplicating consent logic. Keep compensation, reward, bonus, approval, rejection, and
return execution outside Parlando.

Tradeoffs: The provider-contract gate and restart work make the change larger than a completion-code
integration, but they prevent a simple researcher workflow from depending on silent duplicates,
moving deadlines, or manual interpretation of asymmetric outcomes. A narrow reconciliation model
adds operational provider state without importing compensation concepts. The clean schema change
requires an adjacent backup and a rehearsed one-off conversion; it avoids permanent legacy readers,
dual writes, and a runtime migration ladder.

## 2026-08-27: Implemented Prolific as verified admission plus factual outcomes

Context: The old runtime trusted launch identifiers, generated completion codes, used coarse
recipient outcomes, and reused mutable activity timestamps as several different deadlines. It also
classified role B as direct and retained too little phase data to review an unmatched wait or a
partner disconnection.

Decision: Prolific is now an installation-level provider connection. Its API token is a protected
game secret and its workspace ID is a shared game setting. Experiment JSON contains only enablement,
study ID, and exactly five distinct researcher-copied completion codes. Activation reads the study
and checks its workspace, external URL, durations, codes, and exact actions. Secure external URL
launches are verified as RS256 JWTs against Prolific's daily-cached JWKS and bound to issuer,
audience, expiry, workspace, and study; the fallback retrieves and checks the claimed submission.
The submission ID is the atomic resumable-admission key, and launch tokens are never stored.

Both humans retain `identity_provider = prolific`. Recipient outcomes use ten factual values. A
reconnect expiry gives the disconnected participant `connection_lost` and the participant who
remained `partner_left`. Idle expiry means there was no accepted message or game action; heartbeats
are transport liveness only. Waiting, lifetime, idle, disconnection, and reconnect timestamps are
durable server facts. The dashboard displays unsuccessful waiting as a timestamp difference and
shows who disconnected and who remained. Parlando contains no compensation rate, amount,
automatic bonus call, or payment work queue.

Administrator session inspection performs narrow read-only provider reconciliation and persists
only submission status, entered code, return-request time, and reconciliation time. Reward and
bonus representations are absent.

Tradeoffs: Activation and admission fail closed when Prolific cannot be verified. The adapter is
deliberately narrower than the provider API, so payment and submission mutations remain manual in
Prolific. Historical event rows lack wall-clock event timestamps; converted last-meaningful-activity
times were therefore reconstructed from `started_at` plus the greatest stored game-clock position
among accepted conversation-message and game-action events.

Process restarts use the simplest deterministic policy: before serving the experiment, every
durable forming or running session left by the previous process is atomically finalized as
`technical_failure`, retaining its participants and giving Prolific participants the approving
Technical-failure handoff. Replaying arbitrary game implementations, voice state, and in-flight
browser readiness would be less reliable than this explicit infrastructure outcome. Re-admission
with the same Prolific submission reaches the stored terminal result through the existing resumable
admission and terminal-state lookup.

Populated workspace conversion: Converted
`games/great-tree/server/parlando-great-tree.sqlite` from schema 13 to 14 after confirming with
`lsof` that it was not open. The adjacent recovery copy is
`games/great-tree/server/parlando-great-tree.schema13-backup-2026-08-27.sqlite`. The conversion
preserved 41 sessions, 82 session participants, and 10 Prolific rows. Existing participant-session
IDs were attached to all Prolific rows and marked `legacy_import`; historical `received_at` serves
as `verified_at` because no stronger old fact exists. Every old `withdrew` result had a game start
and became `left_game`; every old participant `timed_out` result came from an idle timeout and became
`idle_limit_reached`. Historical deadlines use the then-uniform limits of 600 seconds waiting, 1800
seconds idle, 14400 seconds lifetime, and 60 seconds reconnect grace. Post-conversion integrity was
`ok`, the foreign-key check returned no rows, all source row counts matched, and no required phase
timestamp or admission verification field was missing.

## 2026-08-27: Test Prolific through independent programs and an ordinary game setting

Context: Unit tests for completion-path validation did not exercise Parlando's process boundary,
API authentication, JWKS retrieval, signed launch, dyadic matching, recipient-specific outcomes,
or dashboard reconciliation. Embedding a fake provider inside Parlando or the runtime stress tool
would increase production and stress-harness complexity while allowing test-only behavior to share
the implementation under test.

Decision: Store `prolific_api_base_url` in game settings with
`https://api.prolific.com` as its default. Use it for workspace verification, activation preflight,
JWKS retrieval, admission, and reconciliation. The participant completion application remains the
fixed `https://app.prolific.com` destination because it is not the API origin. Provide three
processes outside the production crate: `prolific-mock`, an independent provider/JWT emulator;
`prolific-test-server`, a compiled deterministic game served by the production Parlando runtime;
and `prolific-test-runner`, a scenario coordinator and reporter. The mock owns independent wire
records and signing material rather than importing Parlando's Prolific types.

The runner prints a stable scenario catalogue before execution, reports pass/fail/skip and elapsed
time as work proceeds, writes a complete JSON report and separate child logs, preserves failed
databases, and returns nonzero unless every scenario passed. The initial matrix covers provider
configuration, activation, signed admission, real game-WebSocket pairing, waiting-room departure,
active-game departure, normal dyadic completion, terminal submission re-entry, unsigned
submission-API fallback, dashboard reconciliation, and the absence of provider mutations. The
dashboard scenario checks workflow semantics rather than row presence alone: unsuccessful waiting
duration, shared terminal cause, participant-specific outcomes, and the completed game result.

Every runner invocation creates a new file-backed SQLite database in its temporary directory and
passes that explicit path to `prolific-test-server`. No workspace game database is an input to the
test. Successful runs remove the temporary database unless `--keep` is selected, while failed runs
preserve it for diagnosis. The Great Tree schema conversion recorded below was application schema
maintenance required by the new game setting, not test setup or a runner dependency.

Lifecycle deadlines are inspected once per second, separately from the 60-second credential and
ticket cleanup. This bounds waiting, idle, lifetime, and reconnect enforcement lag without running
the heavier credential cleanup on every tick. WAIT-02 verifies that deadline expiry selects the
same Unmatched handoff as an active waiting-room departure.

Tradeoffs: A configurable provider origin means a game administrator can direct the protected API
token to another compatible endpoint; this is within the same trust boundary as setting the token
itself. Remote endpoints should use HTTPS, while loopback HTTP permits deterministic local tests.
The emulator proves behavior against the represented contract, not Prolific production, so a real
test-participant run or small pilot remains a release and study-operations check.

Follow-up workflow expansion: Treat the standalone matrix as an executable specification, not only
as a passing smoke test. The expanded catalogue names ordinary and adverse paths separately across
consent, recruitment-source isolation, unmatched waiting, assigned-but-not-started sessions,
waiting and running reconnects, explicit departure, normal completion, inactivity, lifetime,
terminal replay, races, dashboard evidence, and provider safety. Consent distinguishes missing,
explicitly declined, and accepted decisions so one failure cannot hide the other cases. Inactivity
distinguishes heartbeats, accepted activity, rejected activity, and no activity. Race checks accept
either valid winner but require one coherent recipient-result family.

The runner intentionally exits nonzero for contractual gaps. The initial expanded run showed that
missing and declined required consent did not block a Prolific participant from matchmaking, and
simultaneous reconnect expiry could label one disconnected role `partner_left`. Neither behavior
was encoded as a passing compatibility contract; the assertions remained red until the runtime
corrections below.

Resolution: Required consent now gates session creation for every human participant source. The
browser remains the ordinary consent interface, while the server rejects missing or explicitly
declined required decisions even when Prolific intake is called directly. Positive reconnect-grace
expiry now invokes the room-wide expiry transition rather than treating one socket callback as an
explicit actor departure. Recipient outcomes are derived from the connection state of every role
at the terminal snapshot: a single disconnected role receives `connection_lost` while its connected
partner receives `partner_left`; if both roles are disconnected, both receive `connection_lost`.
The existing three failing workflow assertions became regression guards without being weakened.

Implementation finding: The first process-boundary run exposed that activation appended the
experiment path to a `public_base_url` which was already experiment-scoped, producing
`/e/<id>/e/<id>/participant`. Preflight now appends only `/participant`; ACT-02 retains this as a
regression check.

Populated workspace conversion: After confirming with `lsof` that it was not open, converted
`games/great-tree/server/parlando-great-tree.sqlite` from schema 14 to 15. The adjacent recovery
copy is `games/great-tree/server/parlando-great-tree.schema14-backup-2026-08-27.sqlite`. The only
new representation is the non-null `game_settings.prolific_api_base_url`, initialized to
`https://api.prolific.com`. Before and after conversion there were 4 experiments, 49 participants,
41 sessions, 82 session participants, and 10 Prolific submissions. Post-conversion integrity was
`ok`, the foreign-key check returned no rows, the schema version was 15, and the singleton game
setting contained the production default. `space-game/.local/parlando.sqlite` remains at its
pre-existing unsupported schema 10 and was not folded into this unrelated schema-14-to-15 change.
# 2026-08-28: Remove obsolete dashboard bindings with their settings controls

## Context

Removing the global Prolific workspace controls left two startup-time DOM bindings in the embedded dashboard script. One obsolete input was included in the list that registers dirty-state listeners, so its `null` value aborted all dashboard initialization.

## Decision

Remove the obsolete workspace bindings and listener target together with the controls. Keep a dashboard HTML regression assertion that rejects either removed JavaScript identifier.

## Tradeoffs and risks

The assertion directly protects this removal but is not a general browser-DOM consistency checker. Dashboard changes still require a JavaScript syntax check and a loaded-page smoke test.
# 2026-08-28: Separate the Prolific experimenter guide from the integration reference

## Context

`docs/prolific-integration.md` explains provider contracts, outcome mappings, signed admission,
restart behavior, and privacy boundaries. That information is useful to maintainers but obscures the
ordered task an experimenter must complete across the Parlando and Prolific dashboards.

## Decision

Keep the existing document as the technical reference and add `docs/using-prolific.md` as the
experimenter-facing procedure. The new guide follows the setup dependency order: shared API token,
inactive-safe participant URL, Prolific study and completion paths, experiment link and validation,
testing, official intake, and review. Link both documents from the documentation index and link the
reference back to the guide.

## Tradeoffs and risks

Some Prolific labels may change independently of Parlando. The guide therefore names the current
controls needed for the task and links to Prolific's maintained API documentation. Maintainers must
update the guide when either dashboard changes its setup workflow.
## 2026-08-31: Coordinated releases require operational integration and post-publication gates

Context: The 0.4.0 release's package and unit-test gates passed before the dedicated Prolific and
runtime-stress suites were run. The stress suite then exposed a missing-heartbeat defect in its
human-human harness. npm publication also required interactive browser approval, registry visibility
lagged the successful upload, and first-party registry lockfiles could only be truthfully refreshed
after the immutable npm version existed.

Decision: Make the Prolific matrix and short deterministic human-human and human-agent stress runs
mandatory release gates. Audit the stress documentation for obsolete CLI flags. Check npm identity
immediately before publishing, keep browser approval attached to the live TTY process, poll registry
state independently, and verify both packages through clean external consumers. Model releases with
deferred registry lockfiles as two commits: a clean artifact candidate followed by a completion
commit containing only registry-derived consumer metadata, with publishable package trees unchanged.
Tag the completion commit and retain the candidate commit as artifact provenance.

Tradeoffs and risks: The release takes roughly an additional minute plus compilation time and may
require a second commit after publication. In return, the gate now covers recruitment behavior,
runtime liveness and audio transport, real registry resolution, and the final first-party lock state.
Short stress runs detect contract and transport regressions but do not replace longer capacity tests;
run longer presets when changes affect concurrency, resource limits, or provider throughput.
## 2026-09-01: Great Tree names its interactive server as the default binary

Context: Great Tree contains both the participant-facing server and a headless agent-experiment
binary. Its convenience runner used an unqualified `cargo run`, which became ambiguous once both
binaries existed and prevented the documented example from starting.

Decision: Set `package.default-run` to `parlando-great-tree`. Ordinary `cargo run` and `run.sh`
therefore start the interactive game, while the headless workflow continues to select
`agent_experiment` explicitly.

Tradeoff: New binaries do not change the default command. A future replacement for the interactive
server must update the manifest deliberately rather than relying on Cargo's single-binary inference.

## 2026-09-01: Publish one task-oriented manual from repository-owned Markdown

Context: Public guidance was split across the root README and many topical Markdown files. Several
entry points used different example games or described maintainer and test concerns before users had
a game–experiment–session model. The manual should remain in this repository, publish cleanly on
GitHub Pages, and remain reusable inside a later product website.

Decision: Make `manual/index.md` the canonical user entry point and organize a Material for MkDocs
site by conceptual dependency: model, first Great Tree run, game creation, experiment operation,
privacy and Prolific, data operations, then deployment and public API reference. Keep the complete
site source under the top-level `manual/` directory so the earlier `docs/` collection remains a
separate technical-document archive. Use checked-in Great Tree screenshots and a repository-native
SVG concept diagram. Pin the documentation toolchain and build it strictly in a GitHub Pages
artifact workflow; keep all source content and styling under version control in this repository.

Tradeoffs and risks: MkDocs adds a small Python build dependency, but leaves the source as portable
Markdown and produces only static files. A few public reference documents are intentionally copied
into `manual/` so the manual builds without reaching into `docs/`; their supported content must be
kept aligned when either copy changes. The
configured `site_url` assumes the repository is published as `coli-saar/parlando`; a rename or
custom domain requires one configuration edit. GitHub Pages must be set to GitHub Actions once in
repository settings before the workflow can deploy.

## 2026-09-01: Use one Python agent example across live and headless modes

Context: The manual named human–agent and agent–agent operation but did not show how a user attaches
a Python policy. The live server already registers the standard `remote_grpc` factory automatically,
while Great Tree's headless runner registered only its compiled Rust policies.

Decision: Add a small role-polymorphic Great Tree Python policy under `manual/examples/` and use it
in both workflows. Document live human–agent seat assignment explicitly: the human is seat `A`, the
agent is seat `B`, and `crownSeat` maps those seats onto Great Tree roles. Register `RemoteAgent` in
Great Tree's headless runner and provide a checked-in Python-versus-Root-Bot YAML run. Keep provider
credentials in the Python process; dashboard/YAML `config_yaml` remains non-secret policy settings.

Tradeoffs and risks: Registering the remote factory adds no external connection unless a run selects
it. The example policy is intentionally deterministic and game-specific; it demonstrates attachment
and lifecycle semantics rather than a general dialogue model. Local loopback HTTP is suitable for
development only. Remote attachment requires TLS, an allowed hostname, and matching transport
tokens, and production concurrency must be load-tested against the Python service. The Python SDK
now stops its gRPC server inside the live event loop during cancellation, which makes the documented
`Ctrl-C` shutdown path clean while retaining the same serving and authentication behavior.

## 2026-09-01: Organize the manual around contracts and research decisions

Context: The first manual draft used Great Tree as the organizing subject well beyond the opening
tutorial. Although concrete, this made general Parlando concepts read like properties of one game
and turned later chapters into sequences of example-specific controls. Communication and agents
were also combined even though they change different parts of a study.

Decision: Keep the opening encounter and first study run example-led, then organize the guide by
stable contracts: task semantics and information, automated-controller boundaries, headless run
specifications, communication data paths, immutable experimental conditions, temporal obligations,
privacy data flows, recruitment trust, research evidence, and stateful deployment. Mark Great Tree,
Python, Linux, and Render material as illustrations or realizations of those contracts. Split
communication and speech into its own chapter, lead agent-only studies with a generic normalized run
specification, and make the navigation follow conceptual dependency rather than repository layout.
Describe privacy as the fixed versioned storage contract implemented by Parlando: minimize core
records through task design, and treat typed-message controls, transcription, and other modalities
as data-path choices rather than inventing per-category storage switches.

Tradeoffs and risks: Principle-led chapters require readers to map the abstractions onto their own
study, but avoid implying that an example's domain choices are platform requirements. Concrete
commands remain where they test an attachment or packaging path. Maintainers should update the
general contract first when behavior changes, then verify that each illustration still exhibits it.

## 2026-09-01: Make the manual a section of a repository-owned Hugo website

Context: The manual was prepared as a standalone Material for MkDocs site, but it is intended to
become part of a larger Parlando website. Keeping MkDocs would make the eventual product pages and
manual separate builds with separate navigation, search, templates, and styling. The repository's
top level is already crowded, and `docs/` is the preferred parent for both public web material and
future internal subdirectories.

Decision: Supersede the standalone MkDocs site with one Hugo project rooted at `docs/web`. Store the
manual as the `/manual/` content section under `docs/web/content/manual`, publish examples through a
dedicated static mount, and leave the older technical documents elsewhere in `docs/` unpublished.
Use one repository-owned light theme with a shared header and footer, manual-specific navigation,
per-page contents, responsive layout, figures, callouts, and a generated local search index. Keep
the visual layer small enough that future product sections can add their own layouts without
changing the manual's content model. Publish the whole Hugo output through the existing GitHub
Pages artifact workflow and pin the extended Hugo version used for local verification.

Tradeoffs and risks: Owning the layouts and CSS requires more maintenance than adopting a packaged
documentation theme, but avoids coupling the future product site to a documentation-first visual
system. Hugo front matter, shortcodes, and navigation data make the content less generator-neutral
than plain Markdown. The manual had not yet been published, so its final `/manual/` URLs are
established directly without redirect files. The earlier MkDocs decision remains above as history;
this decision supersedes its generator, source location, dependency, and deployment details.

## 2026-09-01: Explain the execution architecture before the runnable example

Context: The manual opened with Great Tree before establishing what Parlando itself contributes.
A new reader could therefore encounter installation commands without knowing that one game combines
a reusable Rust server runtime, a game-specific Rust module, a reusable JavaScript client, and a
game-specific React interface across server and browser boundaries. The relationship between two
human browsers and optional external agents was also distributed across later chapters.

Decision: Add **What Parlando is** as the first manual chapter and make it the primary landing-page
path. Use one checked-in, accessible SVG to show the Rust server, two browser seats, reusable and
game-specific layers, the authoritative state boundary, SQLite, an optional compiled Rust agent,
and an optional external agent service. Keep the prose organized by execution boundary and state
authority, then connect that architecture to the later game/experiment/session model. On narrow
screens, preserve diagram legibility with horizontal scrolling instead of shrinking labels below a
readable size.

Tradeoffs and risks: The illustration intentionally summarizes several protocols rather than
enumerating endpoints or deployment processes. It represents one live two-seat session; headless
agent batches reuse the game and agent contracts but do not instantiate the browser or live-study
path shown. Future changes to seat count, state authority, remote-agent transport, or client/server
ownership must update the chapter and SVG together.

## 2026-09-01: Present speech as composable study paths

Context: The manual's communication chapter named typed messages, voice relay, transcription, and
text-to-speech, but treated them mostly as provider and deployment concerns. It did not give users a
complete model of the interaction modes, the participant interface, or the path by which human
speech becomes an agent message and an agent message becomes audible speech.

Decision: Recast the chapter around four independent paths: durable typed messages, ephemeral live
audio, final transcripts that enter the ordinary conversation, and synthesized presentations of
already-stored agent messages. Begin with a fully spoken human–agent turn, provide a study-mode
matrix, and then connect the model to game-interface responsibilities, dashboard configuration,
provider trust boundaries, durable evidence, readiness, failure, and deployment. Add an accessible
SVG whose visual grammar distinguishes stored text, ephemeral audio, and external providers. State
the Parlando 0.4 limitation that Speechmatics partial hypotheses can be requested but are neither
displayed nor stored.

Tradeoffs and risks: The chapter is longer because communication affects study design, user
experience, privacy, data interpretation, and operations at once. The mode matrix keeps those
consequences comparable without making one example the organizing subject. Provider names and the
partial-transcript limitation describe the current implementation and must change when additional
providers or provisional-caption delivery become supported.

## 2026-09-01: Foreground the concrete game-author deliverable

Context: The manual explained the authority and information boundaries of a game before showing the
artifact an experimenter must produce. Readers could learn what `State` and `Observation` mean
without yet knowing that an ordinary game consists of a Rust crate and a compiled React application,
how those components connect, or where Parlando removes implementation work.

Decision: Introduce the experimenter-owned deliverable in the first architecture chapter and at the
start of the game-building chapter. Describe a conceptual project tree and two build products: one
Rust executable containing the authoritative game and one set of static participant assets. Define
their common boundary as corresponding serialized actions, observations, and completion rather than
implying that Rust and TypeScript literally share source types. Organize the implementation path
into six passes from task specification through a configured pilot, and present the game-creation
skill as the normal scaffolding route. State explicitly which live-study facilities remain owned by
Parlando.

Tradeoffs and risks: The conceptual tree uses the Great Tree repository layout to make the artifact
tangible but does not prescribe exact file names. The two language implementations must continue to
agree on their JSON shapes; Parlando 0.4 does not generate TypeScript types from Rust definitions.
If a future release adds schema or client generation, the described authoring workflow should be
simplified rather than preserving the current duplication as a permanent contract.

## 2026-09-01: Write the manual for sequential reading and actual operation

Context: The manual established Parlando's concepts and capabilities but often stopped before the
reader could perform the corresponding work. An attempted task-chooser landing page treated the
manual as a lookup portal for already knowledgeable users, which conflicted with its intended role
as a coherent, self-contained introduction.

Decision: Treat the manual as a book read in navigation order. Establish architecture, install the
example, define games/experiments/sessions, and run one complete pilot before introducing game
authoring. Build the authoring chapter as an end-to-end Rust and React construction walkthrough,
then extend that same boundary with agents, headless runs, and speech. Only afterward configure a
real condition, consent, Prolific, data operations, and deployment. Put commands, dashboard actions,
expected outcomes, and failure interpretation inside the explanatory sequence rather than in a
separate quick-start layer. Add actual Parlando 0.4 dashboard screenshots for configuration,
declarations, privacy review, agent selection, speech, Prolific, provider settings, and export.

Tradeoffs and risks: Sequential exposition repeats a small amount of context at chapter boundaries
and is longer than a lookup-oriented guide, but it does not assume prior Parlando expertise. The API
and migration reference remain suitable for selective lookup. Screenshots improve procedural
orientation but must be regenerated when dashboard labels or layout change; captions and prose must
still carry the essential instruction for accessibility and future UI changes.

## 2026-09-01: Document supported communication configurations, not switch combinations

Context: The communication chapter inferred a catalogue of study conditions from independently
represented voice, transcription, and synthesis fields. In particular, it advertised live
human–human voice without transcription and several partial human–agent variants. The runtime can
represent low-level combinations that are not thereby defined, tested, or supported as product
conditions. The JavaScript client also exposes conversation data and `sendMessage`, but version
0.4.0 supplies no generic React chat widget.

Decision: Document three complete user-facing interfaces only: game-authored typed dialogue,
human–human speech with transcription, and human–agent speech with transcription and synthesized
agent replies. Describe typed dialogue as React work performed by the game author over the client
API. Remove the inferred condition matrix and the voice-without-transcription presentation from the
speech diagram, privacy table, pilot procedure, and surrounding manual chapters. Treat individual
dashboard fields as implementation controls within the documented configurations, not as evidence
that every combination is a supported condition.

Tradeoffs and risks: The server's transport documentation and tests may exercise component behavior
outside these complete configurations; that remains useful implementation evidence but is not a
user-facing product promise. If another communication configuration becomes supported, it should be
added to the manual only after its participant interface, validation, end-to-end tests, data
contract, and operating procedure are all defined.

## 2026-09-01: Encapsulate Parlando internals in the experimenter manual

Context: The manual exposed implementation mechanisms such as audio encoding, network protocols,
connection tickets, queue behavior, database event representations, hashes, and internal agent
identity machinery. These details explained how Parlando implements its guarantees but did not help
an experimenter design a task, configure a condition, recruit participants, interpret data, or
respond to a failure. They increased cognitive load and made internal choices look like public
configuration surfaces.

Decision: Organize the manual around four experimenter roles: study designer, game or agent author,
data-collection operator, and deployment operator. Keep public Rust, React, Python, command-line, and
dashboard interfaces when the reader must use them. Keep an implementation constraint only when it
changes a supported deployment, privacy statement, data interpretation, or recovery procedure.
Describe all other mechanisms through the user-visible guarantee they provide. Mark the German
privacy-review documents as specialist appendices rather than part of the ordinary reading path.

Tradeoffs and risks: Public API reference and the 0.3-to-0.4 database conversion remain technical
because they are themselves user tasks. Deployment chapters still name SQLite, HTTPS, environment
variables, and one-instance operation where the operator must configure them. Lower-level protocol
and storage details continue to belong in maintainer documentation and tests; if an internal choice
later becomes a supported extension point, it should return to the manual with a concrete user task.

## 2026-09-02: Isolate live client/server contract tests from language-local suites

Context: The runtime had broad Rust HTTP and WebSocket coverage and the JavaScript package had
component-level lifecycle coverage, but no explicit suite owned the complete live boundary. One
valuable audio mute contract spawned Node from an ignored test inside the publishable Rust crate.
It ran through the repository Make target but disappeared from ordinary Rust test results, and its
location coupled the Rust crate's tests to a sibling JavaScript build.

Decision: Add a non-publishable `client-server-tests` crate as the owner of live cross-language
contracts. Give it a deterministic, semantically narrow dummy game with accepted, rejected, and
completing actions, while allowing the scenario matrix to be as broad as the participant lifecycle
requires. Exercise configuration discovery, consent, waiting, pairing, role-specific observations,
messages, actions, completion, reconnect behavior, and deadlines with ordinary Rust HTTP and
WebSocket clients. Move the production JavaScript audio-sink contract into a separate audio module
in this suite and run its lightweight Node driver without a browser automation dependency. Keep
server-only protocol tests in `rust-server` and JavaScript-only rendering tests in `js-client`.

Tradeoffs and risks: The lifecycle scenarios validate the public wire contract rather than React
rendering or browser-engine behavior; existing JavaScript rendering tests retain that responsibility.
The audio scenario still requires Node and a built `js-client`, but the dependency is now explicit
in `make test-client-server` and never represented as an ignored Rust unit. Short real server
deadlines keep timeout behavior realistic but can be more timing-sensitive than an injected clock;
condition-based polling and generous outer deadlines bound that risk. If timing cases become flaky,
the next step is a runtime clock abstraction rather than longer fixed sleeps.

## 2026-09-03: Make the production JavaScript client own contract-test participation

Context: The first version of the live contract suite used Rust HTTP and WebSocket helpers for the
participant lifecycle. This proved that the production server honored its wire contract, but did
not prove that the published JavaScript client's request mapping, credential handling, session-plan
mapping, WebSocket URL construction, message decoding, or send methods agreed with that contract.
Only the audio-sink scenario crossed the JavaScript–Rust boundary.

Decision: Remove the Rust participant client from `client-server-tests`. Keep Rust responsible for
constructing an isolated real server, activating it through the administrator API, and selecting
scenario-specific server configuration. Run all participant operations inside Node with the built
production `ParticipantClient`; use its HTTP methods, one-use game and audio plans, WebSocket URL
construction, protocol decoder, and action and message send methods. Pass only the loopback origin
and scenario name into Node over standard input. Participant credentials are created and retained
inside JavaScript and never cross the process boundary. Use Node's `ws` implementation for the
browser WebSocket primitive and simulated Web Audio primitives only for hardware-facing audio code.

Tradeoffs and risks: This suite now detects drift between the JavaScript package and Rust server,
but it intentionally does not render React or reproduce browser enforcement of CORS, mixed-content,
cookie, and origin policies. React rendering remains covered by JavaScript component tests, while
deployment browser-policy checks remain operational tests. Reconnection is driven with the same
fresh-ticket and reconciliation methods used by the participant application, but the React hook's
backoff timer itself remains a source-level JavaScript concern. Node processes are killed when the
Rust owner times out, and fixture input is closed explicitly so a stalled driver cannot leak into
later tests.

## 2026-09-23: Validate Prolific launches against the experiment entry route

Context: The dashboard and user documentation supply Prolific with
`/e/<experiment-id>/` plus the three provider identity placeholders. The activation preflight
instead required `<public-base-url>/participant`, which is not a participant route and caused a
study configured with Parlando's own displayed URL to fail provider readiness.

Decision: Build the expected Prolific external-study path from the effective runtime public base
URL plus its canonical trailing slash. A hosted experiment runtime has already scoped that base to
`/e/<experiment-id>`; a directly built single-experiment router retains its configured mount base.
Continue to require URL-parameter identity recording and all three Prolific parameter names. Use
the hosted `/e/<experiment-id>/` URL in the provider integration fixture so secure-token audience
checks cover the real participant entry URL.

Tradeoffs and risks: Validation remains deliberately exact before the query string; alternate
paths, missing trailing slashes, and URLs for another experiment are rejected. Experiment IDs are
already restricted to URL-path-safe ASCII characters, so no additional path encoding is needed.

## 2026-09-24: Treat the Prolific public base as a URL prefix

Context: The 0.4.1 Prolific URL correction compared the external study URL's complete path with the
configured public base plus its trailing slash. In installations where the effective public base
is the host root, the documented participant route `/e/<experiment-id>/` is beneath that base but
is not equal to it, so a correctly configured study still failed activation.

Decision: Require the Prolific external study URL to start with the normalized public base and its
trailing slash. The trailing slash remains part of the boundary, so a lookalike host or a sibling
path that merely shares the base's final path segment is not accepted. Query-parameter validation
continues to require all three Prolific identity names independently.

Tradeoffs and risks: Deployments may intentionally place an entry route below the configured
public base. URL ownership is therefore enforced at the configured base boundary rather than by
one exact participant path; administrators remain responsible for configuring that base narrowly
enough to describe the routes owned by this Parlando installation.

## 2026-09-24: Make the server own one exact Prolific setup URL

Context: Prefix validation repaired the immediate mismatch between an installation origin and its
experiment route, but it left three authorities for the same contract. The dashboard built a URL
from its current browser origin, catalogue readiness validated against the installation root, and
runtime activation validated against an experiment-scoped base. Raw substring checks also accepted
wrong paths, constant identities, fragments, and parameter names outside the query.

Decision: Derive every stored experiment's effective public base from the configured installation
origin and `/e/<experiment-id>`. Generate the complete Prolific setup template on the server, expose
it in the experiment catalogue, and require the provider's `external_study_url` to equal that
template exactly. The exact contract includes the canonical trailing slash, parameter order, names,
and literal placeholder values. Use the same effective base for catalogue checks and runtime Start,
which also gives both paths the same provider-preflight cache key. Treat the installation public
URL as an origin without path or query state, and require HTTPS for remote Prolific intake while
retaining loopback HTTP for deterministic integration tests. Do not render an Open action for a
provider template because a literal template is not a valid participant launch.

Tradeoffs and risks: Semantically similar URLs with reordered parameters or additional custom
parameters are rejected. This is intentional because Parlando publishes one copyable contract and
does not consume custom launch parameters. Deployments that previously placed path or query state in
`Server::public_url` must configure their reverse proxy around an origin-only public URL. Local
Preview remains action-scoped: each Open or Copy operation creates a fresh synthetic invitation.

## 2026-09-23: Release the Prolific URL correction as 0.4.1

Context: The coordinated Rust and JavaScript packages were both published at 0.4.0. The Prolific
URL correction changes server validation without changing public APIs, configuration, protocol, or
the database schema.

Decision: Publish the correction as coordinated patch release 0.4.1. Update every first-party
dependency manifest and current-version document together. Refresh Cargo lockfiles against the
repository patch during preparation; defer consumer npm lockfiles until the immutable 0.4.1 npm
artifact exists, then resolve them from the registry and verify their tarball URLs and integrity
digests.

Tradeoffs and risks: Until publication, the first-party client manifests require 0.4.1 while their
lockfiles still record 0.4.0. This is an intentional release boundary, not a supported finished
state; publication is complete only after those lockfiles resolve the registry artifact and the
published release audit passes.

## 2026-09-24: Release the exact Prolific URL contract as 0.4.2

Context: The follow-up Prolific correction makes one server-generated setup URL authoritative,
requires exact provider equality, rejects invalid public origins and insecure remote intake, and
removes the dashboard action that opened a literal provider template. These are compatible fixes
to the 0.4 public API, protocol, configuration schema, and database schema.

Decision: Publish the correction as coordinated patch release 0.4.2 of the Rust runtime and
JavaScript client. Update every first-party dependency manifest and current-version document in the
same release. Validate all ordinary test and package gates, but omit the opt-in runtime and audio
stress suites for this release as explicitly requested. Publish the Rust crate before the npm
package, refresh consumer lockfiles from the immutable registry artifacts, audit the published
versions, and tag the resulting release commit as `v0.4.2`.

Tradeoffs and risks: The Git history contains a published 0.4.1 package release without a matching
Git tag or GitHub release. This release does not rewrite that history or create a retroactive tag;
`v0.4.2` is the next source tag. Consumer npm lockfiles temporarily remain on 0.4.1 until 0.4.2 is
published, after which their registry URLs and integrity digests must be refreshed before tagging.

## 2026-09-28: Keep the pilot-data corrections provider-neutral

Context: The pilot database exposed two concrete failures. A durable participant admission could
enter a new session after its completed session had been removed from runtime memory because the
join path consulted only the in-memory sessions. Separately, corpus export treated an unstarted
one-participant session as a malformed completed game and rejected the entire export. The standard
client completion message could also disappear when an application supplied additional completion
content.

Decision: Before pairing a participant, consult the durable participant-session record and return
its recorded terminal outcome when one exists. Keep this behavior independent of Prolific: the
provider's session identifier selects the same durable admission, but the server rule applies to
all admission sources. Exclude sessions without `started_at` from corpus export before validating
that each exported session has exactly two participants. Preserve the exact-two check for sessions
that actually started, without repairing or tolerating malformed historical data. Make the
JavaScript client always render its standard recorded-outcome message, with application-provided
completion content remaining optional supplementary content. Summarize existing session-end causes
in the dashboard using the sessions already loaded there. Do not change the `Game` interface, the
session state machine, the database schema, or public protocol shapes.

Tradeoffs and risks: A malformed session that did start still blocks corpus export, making durable
inconsistency visible instead of silently omitting it. The dashboard visualization describes only
the currently shown sessions and reports Parlando's session-end causes, not game-specific wins or
losses. Re-entering with a new admission remains valid; only reuse of the same durable admission
returns its recorded result.

## 2026-09-28: Separate experiment progress from session inspection

Context: The session-end visualization was initially placed above the session catalogue. That made
the catalogue denser and coupled the visualization to the session list's lifecycle filter, even
though the chart is an experiment-level summary rather than a session-selection control.

Decision: Give experiment progress its own tab next to Sessions and render one horizontal stacked
bar whose segments represent Parlando's durable session-end causes. Load an independent,
lifecycle-neutral aggregate directly from every durable session in the experiment so changing the
Sessions filter does not change the Progress chart. Keep the Sessions endpoint bounded for session
inspection, but do not use that bounded list as progress data. Keep counts in a legend and expose
the count, total, and percentage through keyboard-accessible tooltips on both bar segments and
legend items.

The progress query also returns one lightweight row for every durable session, ordered newest first
so the oldest session remains at the bottom. The log is deliberately unpaginated and scrolls with
the Progress panel. Each row links to the existing Sessions detail view. Testing sessions remain in
this chronological record with muted text, but aggregates and recruitment targets count research
sessions only. This preserves operational visibility without letting local previews alter study
progress.

An experiment may optionally configure `progress.planned_sessions` for direct or mixed recruitment.
When it is absent and Prolific intake is enabled, derive the target from the linked study's current
`total_available_places`: divide by two for human-human experiments and by one for human-agent
experiments. Do not round an odd human-human place count; omit the target box and report the
configuration issue instead. A configured target takes precedence because Prolific places cannot
describe participants recruited outside Prolific.

Refresh the Prolific study facts after each newly created research session and through an explicit
Progress-tab action. Keep the last successfully verified facts when a refresh fails, and do not
delay session creation on the provider request. Provider refresh updates runtime metadata rather
than creating experiment-configuration revisions. This adds no database migration and changes no
session-state transition.

When a target exists, show it as a bordered white capacity box filled by completed and active
research sessions. Unsuccessful ended sessions do not consume planned capacity; show them to the
right as recruitment overhead. Completed or active sessions beyond the target also extend to the
right, so progress may visibly exceed 100% without compressing failures into the planned target.
When no target is available, retain the ordinary proportional outcome bar and do not draw a target
box.

Tradeoffs and risks: Returning all lightweight rows makes the Progress response grow linearly with
the experiment, as explicitly chosen for an unpaginated scrollable log. It still omits participants,
events, and game state. The colors distinguish Parlando lifecycle outcomes; they do not introduce
or infer game-specific win/loss semantics.

Visual verification with a disposable Great Tree database exposed one dashboard-shell issue: the
generic `.secondary` button rule could override the browser's `hidden` presentation, leaving the
Prolific refresh action visible for direct-only experiments. Give the HTML `hidden` attribute one
global, important display rule. Serve embedded dashboard CSS and JavaScript with `Cache-Control:
no-store` so a restarted server cannot combine a newly compiled dashboard module with an older
cached stylesheet. This is limited to the protected administrator shell assets.

Keep a reusable disposable-data script in `notes/progress-demo.sql`. It deliberately combines a
configured six-session target, completed sessions, active sessions, multiple terminal failure
causes, and muted testing sessions. The script is for manual visual verification against an empty
`progress-demo` experiment after the disposable server has started; applying it earlier would
correctly cause startup recovery to terminate its artificial open sessions. Two small companion
scripts switch between the fully filled target and a partially filled target without recreating the
fixture. These are not supported database migrations or production fixtures.
Load progress during initial dashboard setup, whenever the Progress tab opens, and while that tab is
visible during periodic refresh. Render loading and request-failure messages inside the chart panel
so a missing request cannot leave an unexplained empty rectangle.

Keep the tab visually consistent with the dashboard's other experiment views: one section heading,
compact totals, the chart, and its legend. Do not add explanatory subtitles or a prose footer when
the labels and totals already state what the visualization contains.

Draw the planned-session boundary above the colored fills so the white capacity box remains visible
as it fills. When outcomes extend beyond the target, separate them from the full-height target box
with a small gap and render them as a shorter bar. This keeps recruitment overhead visible without
giving it the same visual weight as planned progress. Make each session-log row the link to
its Sessions detail rather than nesting a link inside it. Use a twelve-pixel outcome stripe, align
all names to a fixed column, render research-session names in the dashboard's normal text color,
and distinguish testing sessions only through muted row text and an accessible label instead of a
repeated visible “Testing” suffix.

## 2026-09-28: Test Prolific returns at the admission boundary

Context: Provider-neutral tests covered rejoining an in-memory session and recovering an ended
session from durable storage, while storage tests covered repeat participation. They did not prove
that a repeated Prolific launch traversed `/api/participants` and preserved the same semantics.

Decision: Add end-to-end admission tests for one Prolific participant returning with (1) a new
`SESSION_ID`, (2) the `SESSION_ID` of an ended session after runtime cleanup, and (3) the
`SESSION_ID` of a running session. A new `SESSION_ID` remains a new admission and may create a new
session. Reusing a `SESSION_ID` returns the original running or ended session. Keep this behavior
provider-neutral below the Prolific admission boundary and make no schema or state-machine change.
Use Prolific's sunrise mark for the participant disclosure control, and explain in the tooltip that
clicking reveals the study, submission, and provider-check details. Completion-code help text names
the game condition first, the resulting Prolific action second, and a suggested researcher-facing
Prolific label third. Labels are suggestions only: Parlando selects paths by configured completion
code and never reads or matches their Prolific display names.

Tradeoffs and risks: The end-to-end tests use Parlando's synthetic local-testing launch contract,
so they exercise admission identity and session routing without making live Prolific API calls.
Provider signature and submission verification remain covered separately. The dashboard icon is a
compact monochrome rendering because the 17-pixel disclosure control cannot accommodate Prolific's
full wordmark. Each protected-endpoint test authenticates its own administrator; it must not depend
on credentials placed in shared test state by another concurrently running test.

The Prolific disclosure tooltip uses three short labeled lines—participant, check status, and click
action—rather than prose. The shared dashboard tooltip preserves explicit line breaks, while its
plain-text content and `aria-label` remain equivalent for keyboard and assistive-technology use.

## 2026-09-28: Keep unsuccessful waiting time with session state

Context: An unsuccessful waiting duration was rendered as a sixth top-level session fact. On the
five-column summary grid this wrapped beneath “Session state,” making its label and value look like
a detached second row.

Decision: Keep the duration as compact secondary text inside the Session state value. Render
“Unsuccessful wait” in the dashboard's muted small text and give the duration normal-text emphasis.
The End reason remains a separate fact because it describes why the session ended, while the wait
duration qualifies the ended state. The session catalogue retains its existing inline “waited”
suffix.

## 2026-09-28: Align Prolific target refresh with the progress chart

Context: The Prolific refresh action was a wide text button aligned to the full Progress panel,
while the chart itself has a narrower maximum width. The unrelated right edges made the action look
detached from the data it refreshes.

Decision: Constrain the Progress header to the chart width and render the action as a compact refresh
icon at its right edge. Its accessible name and tooltip are both “Refresh session target from
Prolific.” Keep the control hidden when the target is not sourced from Prolific, as before.

## 2026-09-28: Use one session-outcome palette throughout the dashboard

Context: Progress bars and chronological stripes distinguished each terminal cause, but status dots
in the Sessions view and participant cards reused broad lifecycle colors. Completed sessions could
therefore appear green in Progress and blue in Sessions; connection loss, no partner, timeouts, and
technical failures also changed color between views.

Decision: Define the session-outcome palette once as CSS custom properties and use those properties
for chart segments, legends, chronological stripes, session-catalogue dots, summary badges, and
participant outcomes. The palette is: completed green, participant left red, connection lost orange,
no partner ochre, inactivity timeout gray, duration limit blue, and technical failure purple. A
generic lifecycle value of `ended` remains blue because it does not identify a terminal cause; when
a durable end cause is available, the session catalogue now uses that cause's status class instead.

## 2026-09-28: Separate Progress dates and times

Context: Each Progress row formatted its creation timestamp as one locale string, which joined the
date and time with punctuation and made clock values begin at different horizontal positions.

Decision: Render the localized date and clock time in separate columns shared by every row through
CSS subgrid. Size the date and time columns to their widest contents and use one uniform gap before
the date, time, and session name. Use tabular numerals and prevent wrapping so times align
vertically. Retain the original ISO timestamp in each semantic `time` element's `datetime`
attribute.

## 2026-09-28: Preserve the Progress DOM between unchanged polls

Context: The five-second Progress poll replaced the chart with a loading message before every
request and then rebuilt both the chart and session log even when the response was unchanged. This
caused visible flicker and disrupted hover and focus state.

Decision: Show “Loading…” only before the first Progress response. Retain the last successful view
during later requests and transient failures. Compare the complete response projection with the
last rendered projection and rebuild the chart and log only when it changes. Reset that comparison
when the user selects another experiment. Manual Prolific refresh uses the same update path.

## 2026-09-28: Keep waiting and post-completion guidance in the shared participant client

Context: The shared participant client already received the server-owned waiting start and deadline
and rendered a countdown, but its explanation exposed the internal Prolific completion-path model
and did not mention waiting compensation. Normal game completion immediately disconnected the
audio transport, leaving two human participants no time to react or say goodbye.

Decision: Keep both experiences in `js-client` so games require no code changes. The waiting screen
explains the concrete consequence of leaving or reaching the deadline. Only Prolific intake mentions
returning the submission and researcher-issued payment for waiting; Parlando does not promise or
issue that payment. On normal completion of a voice-enabled human–human session, commit the terminal
result immediately and attach a server-owned deadline after the configured farewell interval
(60 seconds by default) only when both audio roles are connected. Keep partner audio relay available
until that deadline while rejecting game input
and suppressing transcription. The terminal client renders the countdown, mute control, and an
explicit End voice chat action. All other terminal causes and human–agent sessions close voice
immediately.

Tradeoffs and risks: The farewell period is transient transport state, not a new session lifecycle
state or durable database value. A server restart therefore ends it early without affecting the
recorded result. The server, rather than the browser timer, enforces the deadline so background-tab
timer throttling cannot extend microphone relay. Post-completion audio is relayed but neither raw
audio nor transcript text is stored. The Prolific completion control remains immediately available;
following it ends the optional farewell through normal page teardown.

## 2026-09-29: Real-browser release acceptance around the contract game

Context: The existing contract tests separately exercised the live Rust server, the published
JavaScript protocol client, rendered lifecycle behavior, audio transport, and the Prolific API.
They did not establish that two independent browser contexts could traverse the participant state
machine against one live server. Manual release checks therefore remained necessary.

Decision: `client-server-tests/browser` contains a deliberately small React consumer of the public
`@coli-saar/parlando-client/react` package. A Rust matrix starts the existing `ContractGame` with a
fresh SQLite database for each scenario and drives the built client in independent headless Chromium
contexts. Each scenario configures the existing waiting, reconnect, idle, and maximum-lifetime
fields, using short real deadlines rather than a second test clock. The matrix continues after a
scenario failure and writes `client-server-tests/target/browser-e2e/report.md`. `make test-e2e`
combines this browser matrix with the existing standalone Prolific process-boundary matrix.

Tradeoffs: Chromium exercises real browser storage, fetch, WebSocket, reload, and offline behavior
without making pixel layout part of the contract. Exact countdown formatting remains in fake-timer
unit tests, while the end-to-end suite verifies that configured deadlines cause the expected live
transitions. The first matrix run is allowed to fail: failures are evidence about the current
runtime unless they arise from the harness itself, and assertions must not be weakened to obtain a
green result.

Follow-up risk: The initial browser matrix covers the principal direct-recruitment transitions and
relies on the existing standalone Prolific runner for provider configuration, signed admission,
returning submissions, handoff mapping, and provider-write safety. A later extension can drive the
signed Prolific launch URL through Chromium without changing the production interfaces.

The final implementation also exercises browser-visible `technical_failure` through an intentionally
failing test agent factory, making the browser coverage gate exhaustive over the current participant
outcome and session-end-cause vocabularies. The `make test-e2e` shell coordinator runs the browser
and Prolific matrices even if either fails, then `scripts/write_e2e_report.py` combines their durable
results in `target/e2e-report.md`. Browser execution remains a direct child of the shell because the
macOS application sandbox aborts an installed Chrome process nested below a Python subprocess.

## 2026-09-29: Put precise deadline assertions below the browser acceptance layer

Context: Waiting, reconnect, inactivity, and post-completion voice surfaces all count down from
server timestamps. Exact second-by-second browser assertions are sensitive to scheduler delays and
would make the release gate flaky. Conversely, a component-only test cannot prove that a production
server deadline reaches a mounted client, survives reload, and causes the corresponding terminal
transition.

Decision: Test formatting, boundary changes, zero clamping, recruitment-specific wording, timer
cleanup, and exactly-once farewell shutdown with fake-time JavaScript component tests. Keep coarse
integration assertions in Chromium: the waiting and farewell countdowns must decrease, waiting and
paused sessions must survive reload without extending their deadlines, and the configured server
deadline must end the relevant capability. Retain atomic race, replay, activity, and Prolific
identity cases in the existing process-boundary suites, where synchronization is deterministic.
The browser matrix adds only browser-owned risks: reload recovery, a duplicated tab, and real media
setup and relay through two independent browser contexts.

The `make test-e2e` coordinator also runs the JavaScript client checks, the complete Rust server
suite, and the live client/server contract suite before the two acceptance matrices. It records all
five exit statuses, continues through later layers after a failure, and includes every layer in the
combined report. This prevents an early deterministic failure from suppressing evidence from the
remaining release checks.

To keep the real-media acceptance scenario short, `voice.post_completion_seconds` now configures
the farewell interval. Its production default remains 60 seconds and validation permits 1–3600
seconds. This replaces the previous hard-coded constant without changing the lifecycle state
machine or requiring game code changes.

Tradeoffs and risks: Chromium uses its fake microphone device, so the test proves browser media
permission, AudioWorklet setup, authenticated transport, relay observation, terminal UI, and
deadline shutdown, but not physical hardware quality. Prolific browser parameter capture remains a
deterministic JavaScript protocol test, while the standalone Prolific matrix covers real server
admission, restoration, handoffs, races, and provider safety; duplicating those cases in Chromium
would increase runtime without crossing an otherwise untested boundary.

## 2026-09-29: Never fall back from Playwright Chromium to desktop Chrome

Context: The browser driver previously fell back to the user's installed Google Chrome when the
Playwright-pinned executable was absent. A desktop application launched as a child of Codex's
restricted macOS process aborted in LaunchServices registration before reaching headless startup.
macOS treated every scenario attempt as an unexpected Google Chrome termination and displayed a
crash dialog for each one.

Decision: The E2E driver launches only Playwright's isolated Chromium build. Both browser Make
targets run Playwright's idempotent Chromium installer before building and executing the fixture.
If installation or launch fails, the matrix reports one ordinary test/setup failure; it never opens
or crashes the user's desktop browser.

Tradeoff: The first browser test run must download the Playwright browser and therefore needs
network access. Later runs reuse the versioned browser cache. This is preferable to coupling the
release test to an independently updated personal browser or producing macOS crash reports.

## 2026-09-29: One canonical coordinated release procedure

Context: Package publishing commands existed in public documentation and a release skill, but the
skill required a caller-supplied version, treated long stress workloads as correctness gates, and
always required a migration guide. The repository did not have one task-oriented document joining
version selection, lockstep package preparation, complete correctness tests, npm's interactive web
approval, publication recovery, and downstream migration duties.

Decision: `docs/releasing-parlando.md` is the canonical maintainer procedure and `AGENTS.md` points
future agents to it. A release increments the coordinated Rust and JavaScript patch version by
`0.0.1` unless the maintainer selects another version. The root `CHANGELOG.md` records both packages.
The release candidate must run `make test` and `make test-e2e`, but the runtime stress programs are
opt-in performance evidence rather than ordinary release gates. The game-generation skill and
current user documentation advance with each release. A version-specific migration guide is
required only when downstream games must change code, configuration, deployment, or data.

The release audit therefore accepts releases without a migration guide by default and validates
one when present. `--require-migration-guide` makes the conditional policy explicit for releases
with downstream work. The release skill now follows the canonical document and instructs agents to
keep npm login or publish processes alive while the maintainer completes web approval. The audit
excludes only `client-server-tests/browser/package.json` from registry-consumer version checks
because that unpublished E2E fixture deliberately uses the local JavaScript package under test;
first-party games remain subject to exact registry-version checks.

Tradeoffs: Running both test targets repeats some JavaScript, Rust, and contract checks because the
E2E coordinator is independently useful. The duplicate work is acceptable for an infrequent
release and makes both entry points self-contained. Omitting stress workloads shortens routine
releases but means capacity regressions require a deliberate stress run when a relevant subsystem
changes.

The completion step gives the maintainer an annotated `v<version>` tag command and the exact
single-tag push command `git push origin v<version>`. Release agents must substitute the actual
version and ask the maintainer to run it; a vague instruction to push tags or a broad
`git push --tags` command is not sufficient.

## 2026-09-29: Release the participant-lifecycle and progress corrections as 0.4.3

Context: The coordinated 0.4.2 packages predate the corrected Prolific return behavior, shared
waiting and farewell countdowns, experiment Progress view, and real-browser release acceptance
matrix now present in the repository.

Decision: Publish these changes as the normal patch release 0.4.3 for both `parlando` and
`@coli-saar/parlando-client`. No migration guide is required because existing games receive the
participant UI and runtime corrections through their dependency update, and neither their source
interfaces nor their databases require conversion. Refresh first-party registry consumer
lockfiles only after publication so they record the immutable 0.4.3 tarball checksums rather than
local or invented metadata.

During the 0.4.3 release gate, `make test` exposed that `test-python` used whichever system Python
was active without installing the SDK's declared dependencies. The target now owns a reusable
repository-local virtual environment under `.local`, installs the SDK editable from its
`pyproject.toml`, and runs the suite with that interpreter. This keeps a fresh release checkout
self-contained without changing the developer's global Python installation.

The same gate exposed two stale harness assumptions. The browser setup command could remain alive
after Playwright's exact Chromium installation was already complete, so setup now checks the
executable selected by the installed Playwright package and invokes the installer only when that
file is absent. It never falls back to a desktop browser. The Prolific mock study now includes
`total_available_places`, which became a required production response field when planned-session
progress was added. Combined reports no longer display an older successful browser report when the
current browser layer fails before producing one.

The Prolific runner launches sibling mock and server executables, but `cargo run --bin
prolific-test-runner` rebuilds only the runner itself. The Make targets now build all binaries first,
so source changes to any companion are guaranteed to be present in both focused and coordinated
E2E runs.

One full-suite run exposed a parallel-test collision in the in-process credential helper: random
human-readable participant handles are intentionally not globally unique, but the helper retained
only one credential per handle across all test routers. A concurrent test could therefore replace
another router's credential and cause a spurious 401. The helper now retains every credential for a
colliding handle and tries them against the target router; production credential lookup remains
unchanged.

The post-publication audit originally exempted the browser E2E fixture's local JavaScript
dependency when checking manifests but accidentally inspected the same local link as though it
were a registry consumer when checking lockfiles. The lockfile audit now applies the same narrow
exemption. First-party games must resolve the published tarball and integrity digest; the browser
fixture continues to resolve `../../js-client` so E2E tests exercise the source being released
rather than the most recently published package.

## 2026-09-29: Tighten Rust runtime invariants without changing game or agent APIs

Context: The Rust runtime had accumulated compatibility parsing, optional construction states,
duplicate domain types, and storage fallbacks. Several of those paths could hide corrupt data or
leave a failed agent session partially alive. The same process also represented its global
administrator dashboard as an experiment named `__dashboard__`, even though that route tree is
installation-scoped.

Decision: Preserve the public `Game`, `GameFactory`, `Agent`, and `AgentFactory` interfaces while
making their surrounding runtime states explicit. Agent settings, identity, provenance, timeout,
and invalid-action limit are resolved once before construction. A pending agent carries that
validated data into the session, and every constructed agent reaches the same shutdown cleanup
path even when `start` fails. Remote-agent factories now return an active remote instance after
the create RPC succeeds, so callbacks no longer support a half-created state.

Use `PlayerRole` as the sole seat type and canonicalize JSON through one shared helper. Keep
dashboard event summaries typed until the HTTP serialization boundary. Persist only a
`StoredExperimentConfig` containing revisioned experiment settings; process resources such as the
database URL, listener settings, and provider secrets come from the current installation when a
runtime is hydrated. Router construction uses a closed mode enum for standalone experiments,
mounted experiments, and the installation dashboard. Dashboard construction has no synthetic
experiment identifier and never persists an experiment record.

Use `SqliteExperimentStore` directly instead of a private trait with one implementation. An empty
database is initialized transactionally. A nonempty database must already contain the exact
current schema marker before any schema write occurs; older or unversioned databases require an
explicit one-off conversion. Export treats malformed stored JSON as corruption and returns an
error. Nonhuman participant identity requires the current structured metadata, and scoped exports
always require an experiment identifier.

Tradeoffs: Existing databases below schema version 16 and stored agent rows missing current
identity metadata now fail clearly instead of being interpreted heuristically. This deliberately
removes implicit compatibility behavior. The remote factory performs its create RPC during
`AgentFactory::create`, which can make construction slower, but callers receive either a fully
usable agent or an error and agent implementations need no changes.

The asynchronous construction regression test synchronizes on an explicit notification emitted
when the gated factory is entered. Scheduler yields, polling intervals, and elapsed-time thresholds
do not establish task ordering and are not used as evidence in this test. A separate runtime test
already covers the agent eventually joining after construction, so this test ends after proving
that the HTTP response returned while construction remained blocked.

## 2026-09-29: Remove private Rust indirection without changing extension APIs

Context: A source-level review found private helpers with long positional argument lists, output
parameters, duplicated event-record construction, transport wrappers in domain helpers, redundant
trait bounds, cloning dashboard projections, and request builders that resolved the same remote
agent state twice. These were implementation smells rather than missing runtime behavior.

Decision: Keep the exported `Game`, `GameFactory`, `Agent`, `AgentFactory`, experiment-runner result,
and protocol types unchanged. Inside the headless runner, share one serialized decision input
between trace and trajectory capture, group the values shared by action application in a private
execution context, and return an explicit accepted-or-rejected action result. Build session event
records through one required constructor and let best-effort callers choose to discard its error.
Dashboard bundles borrow typed event summaries until JSON serialization. Internal conversation
commit code returns its domain response directly, while HTTP boundaries remain responsible for
`Json`. Remote decision requests receive an already resolved agent identity.

Tradeoffs: The private execution context is deliberately limited to the runner operations that
actually share its fields; it is not a new public framework or extension point. Borrowed dashboard
bundles have lifetimes tied to the event slice, which is appropriate because they are serialized
within the same call. Focused tests preserve remote affordance presence, provider-secret status
shape, runner lifecycle and trajectory behavior, dashboard bundle output, and conversation-event
persistence.

## 2026-09-30: Preserve complete output for every coordinated E2E layer

Context: A live contract failure was followed by successful browser and Prolific suites. The
coordinator preserved the failing status, but its combined report had no saved contract output,
so the original assertion could not be recovered from artifacts.

Decision: Give each `make test-e2e` invocation a unique directory in `target/e2e-runs`. Stream
each layer's combined stdout and stderr through `tee` into its own log. Bash `pipefail` preserves
test failures through that pipeline, and later layers still execute. Save a run-specific report
and structured exit statuses, and link every log from the latest combined report. Copy the browser
report into the run directory and resolve the Prolific report from this run's announced path
instead of selecting an unrelated newest artifact.

Validation: Deterministic subprocess tests exercise the real Make recipe with a contract failure
and with all layers succeeding. They verify stderr retention, exit-status propagation, continued
execution, report links, and rejection of stale Prolific artifacts. Existing runs cannot recover
output that was never logged; subsequent runs retain it.

## 2026-09-30: Diagnose the audio contract activation race

Context: The audio contract sometimes relays marker 1 to the partner without recording it in
transcription. An instrumented failure recorded only role-B marker 9 and role-A marker 3.
The transcription Ready handler sends its status before marking participant audio readiness
and completing durable game activation. Relay accepts forming sessions; transcription requires
a running, unpaused session. The test treated provider readiness as proof of game activation.

Decision: Retain recorded-frame details in assertion failures. A temporary diagnostic change
waited for authoritative participant state `active` before capture; all ten repeated runs passed.
Remove that temporary change after the experiment. The correction should synchronize capture
with the active game state, rather than delay capture for an arbitrary duration or relax the
server's transcription lifecycle gate. This diagnosis does not change runtime behavior.

## 2026-09-30: Add deliberately failing readiness regression tests

Context: Existing agent delivery tests wait for an internal `agent_started` event after the
public active notification. This can conceal activation preceding usable agent initialization.
The user requested regression tests now, leaving production fixes for a later change.

Decision: Add a recording agent with explicit construction/start gates and callback channels.
Test that blocked start keeps the public session waiting and that the first action and message
after public activation are delivered exactly once, with no internal readiness wait. Exercise
leave during both construction and start, checking disposal and preservation of the terminal
result; setup-time departure must be classified as leaving the waiting room. Strengthen the
existing start-failure test to reject any intermediate public active state and replace its
shutdown polling with a notification.

For audio, preserve the current active-game transcription boundary. Hold activation open by
withholding game connections, verify forming-room relay without transcription, then send the
first frame immediately after the public active notification. A recording transcription peer
reports only after receiving ordered Finish, so neither assertion relies on a sleep. This test
does not claim provider readiness alone means the game is active or weaken the existing JavaScript
mute contract. No public API or production code is changed.

Expected current failures: blocked agent start already advertises `active`; leave during blocked
start yields `left_game` rather than `left_waiting_room`; failed start advertises activation before
ending. Late-construction disposal and the explicit audio lifecycle boundary currently pass.
Handshake and callback timeouts bound failed execution; elapsed time does not establish ordering.

Validation: The full Rust library suite completed with 223 passing tests and exactly the three
expected failures above. Three further focused repetitions produced the same failures each time,
with the audio boundary and late-construction cleanup passing every repetition. Full output is
saved in `target/readiness-tests/full-rust.log`; focused and repeated runs have separate logs in
the same directory. The failing tests are enabled, not ignored or converted into expected panics.

## 2026-09-30: Make readiness follow usable agent and transcription state

Context: The new readiness tests showed that the server activated the game before agent start
completed, misclassified departure during start as `left_game`, and advertised an active game
even when start failed. The JavaScript audio contract also exposed transcription readiness being
published before the durable forming-to-running transition.

Decision: Attach a constructed agent with `ready=false` and give its task ownership of start,
execution, and shutdown. Run start without holding the session transition lock, so leave and
expiry remain possible. After successful start, acquire that lock, verify the session is still
forming, install the inbox, and mark the participant ready. Failed or cancelled setup never
marks readiness and always reaches shutdown. Remove the pending-agent registry and the path
which launched initialization after game activation; initialization now has one owner.

Activation queues an internal Activated event before public input can arrive. The agent waits
for that event and the activation transition lock before making its initial decision. Thus it
cannot act before activation snapshots are published, and immediate participant observations
are queued behind activation rather than silently omitted for a missing inbox. The callback
API for games and agents is unchanged; start receives the forming session's initial observation.

Transcription provider readiness marks internal audio setup complete, but client-facing
transcription readiness requires a committed running, unpaused session. Both audio control
messages and game-channel voice status use that condition. Activation updates all participant
audio sockets, including peers whose provider finished earlier. Forming-room voice relay remains
supported and untranscribed; this is now explicitly distinguished from transcription readiness.
The audio regression checks both false readiness during formation and true readiness after
activation, and the original JavaScript mute contract needs no added wait.

Tradeoffs: Initialization can begin before the human/audio connections finish, but autonomous
decisions cannot. Cancellation during a start callback remains bounded by the configured start
timeout; after it returns, terminal state is rechecked and the agent is disposed of. No database
schema, public trait, or protocol shape changes are needed.

Validation: All 226 Rust library tests pass, including the three previously failing regressions.
The complete `make test-e2e` gate exited zero: 119 JavaScript tests, 8 live contracts, 13 browser
scenarios, and 35 Prolific scenarios pass, alongside the Rust layer and logging regression tests.
The original JavaScript audio mute contract passed ten further consecutive runs with its driver
unchanged. The separate runtime integration suite passed all five tests, including the remote
gRPC agent and Speechmatics streaming contracts. The immutable gate report and all layer logs
are in `target/e2e-runs/20260930-072240-yH5FZj`; focused diagnostic logs are in
`target/readiness-tests`.

## 2026-09-30: Keep the E2E reporter usable with the maintainer's Python 3.9

Context: The E2E gate passed with the agent shell's Homebrew Python 3.14.6, but its logging
prerequisite failed in the maintainer's Conda base environment (Python 3.9.13). The fixture's
report writer used `zip(strict=True)`, introduced in Python 3.10. The resulting subprocess
TypeError was hidden by the fixture subsequently indexing the missing report and raising KeyError.

Decision: Remove the unnecessary strict argument; the reporter already checks the exact CLI
argument count before pairing its five statuses with the five layers. No interpreter switching,
version fallback, or new dependency is needed. Assert report existence with the coordinator's
captured stdout/stderr before inspecting report contents, exposing the original failure if
generation fails again. The success and failure-path logging tests pass on both Python 3.9.13
and 3.14.6. Prior full-gate validation applied only to the agent interpreter; the full gate is
being revalidated explicitly with `/opt/anaconda3/bin/python3`.

The combined report also records the interpreter version and executable, and the subprocess tests
verify that metadata. This makes environment-specific validation evidence visible in the artifact.

Validation: The full `make test-e2e PYTHON=/opt/anaconda3/bin/python3` completed with exit zero:
all five layers pass, including 13 browser scenarios and 35 Prolific scenarios. Its immutable
report at `target/e2e-runs/20260930-073933-Smfa5j/report.md` records Python 3.9.13 and
`/opt/anaconda3/bin/python3`. No Conda environment change is required to run the default command.

## 2026-10-02: Measure ASR granularity and distinguish sample time from game time

Context: Issue #41 requires exposing simultaneous speech without losing utterance text. We ran a read-only credential-backed experiment under `notes/asr-timing-2026-10-02/`, using cached ElevenLabs PCM and real Speechmatics responses, followed by an isolated in-memory Great Tree server probe. Production code and the live workspace database were not edited.

Approach: Retain raw responses before proposing a token representation. Compare unchanged samples under leading silence, startup waits, sample-free pauses, omitted samples, and buffered delivery. Request entity details explicitly to inspect normalized multiword expressions. Keep source sample positions, actual dispatch clocks, and provider times separate. The successful native conversion trials used a transparent loopback relay for real cloud traffic after direct server-to-cloud backpressure prevented reliable paired output.

Evidence: A normal English sentence yielded 14 independently timed words across 11 finals and one utterance end, identically across three repetitions. A money expression otherwise labelled one word exposed six timed spoken words with enable_entities=true. Provider times were stream-relative and advanced with supplied samples. In the paired native trials, source phrase separation was about 5.082 seconds both with silence and with a two-second sample-free pause. Provider word separation was 5.040 versus 3.040 seconds, and both utterances within each case received the same fixed game-clock anchor. Current conversion would therefore preserve the missing two seconds even after adding word results.

Tradeoffs and follow-up: A future implementation should preserve formatted text and ordinary words plus entity spoken forms. It needs a sample-to-source-time correspondence, or equivalent transmitted silence for gaps, before claiming session-aligned token intervals. Utterance metadata included silence and should not substitute for spoken-word intervals. Boundary accuracy remains approximate; the provisional 150 ms target was not established against an independent oracle. Reconnect alignment remains inconclusive after quota/startup failures. Direct Rust-to-cloud backpressure is a separate observed limitation whose cause was not established. Complete evidence and reproduction commands are in the internal report; these observations do not yet define a supported public contract.

Follow-up in the same experiment: A final orderly reconnect trial retained both phrases after waiting for pending transcript commitment and quota release. The old/new stream anchors were 46 ms and 15,670 ms on game time; reconstructed word separation differed from source dispatch by +42.2 ms. This verifies fresh-stream anchoring for that controlled local case. It supersedes the earlier inconclusive reconnect result, while abrupt reconnects, pending-final preservation, provider quota delays, and direct transport backpressure remain limitations.


## 2026-10-02 — Timed speech tokens and capture-clock correspondence (issue #41)

Context: saved Speechmatics responses expose independent word timings, but normalized
entities otherwise collapse spoken words. Stream timestamps count supplied samples,
so a fixed stream-origin addition compresses mute and dropped-frame gaps.

Decision: explicitly enable entities, recursively retain their spoken forms as ordered
word/punctuation tokens, and preserve formatted utterance text independently. Map
provider offsets through successfully queued 20 ms frames using browser capture
coordinates relative to the first accepted frame. Anchor that frame to server receipt
on the game clock; use preceding frames for endpoints and following frames for word
onsets at gaps. Keep point punctuation on the preceding boundary. Normalize dispatch
jitter to non-overlapping frame intervals. Retain the current utterance's frames for
idempotent retries, prune older correspondence, and scope deduplication to the stream
origin. Read provider messages independently from writes and abort the writer when the
receiver closes. An interrupted stream reports failure rather than treating pending
fragments as a completed utterance.

Storage/export: tokens belong to existing conversation-event JSON metadata; no database
schema or conversion is needed. The corpus projects only kind, text, game-clock start_ms,
end_ms, and optional confidence, validates them, and nests them under their utterance.
The dashboard uses A/B bubbles and a shared bounded token axis. Repeated equal-text
utterances remain distinct. Missing historical timings remain unavailable.

Tradeoffs/limits: capture dispatch times and the server-receipt anchor estimate capture
time; initial network latency and ASR boundary error remain. Missing samples and lost
pending finals cannot be recovered. Tests use saved real ASR responses and synthetic
speech PCM against a local protocol peer, without credentials or paid services. Direct
provider reliability under interrupted networks remains a deployment check, not a
conclusion from deterministic tests.

Validation: 236 runtime library tests and one binary test passed; the local Speechmatics
contract replay passed with all generated PCM frames; 17 dashboard tests passed. Browser
inspection of the production renderer with synthetic overlapping A/B turns verified
both views. JSON corpus schema parses, and export tests cover field projection and
invalid intervals. No live provider calls, production database writes, Git operations,
release, or publication were performed for this implementation.


## 2026-10-02 — Complete vertical token view

Context: the initial token view silently filtered the session to a ten-second
window. Horizontal scrolling only traversed that window, so subsequent utterances
were omitted and short words were clipped by duration-scaled bars.

Decision: replace the windowed horizontal plot with all recorded speech tokens
in vertically scrolling chronological rows, sharing a start-time column and
separate A/B columns. Tokens with identical onsets share a row. Every token shows
its endpoint and duration; punctuation remains distinct and parent utterance text
and confidence remain accessible by hover or keyboard focus. Missing historical
token data appears with its utterance in chronological order. Remove the obsolete
start/window controls and horizontal-axis styles.

Tradeoff: row spacing prioritizes legibility rather than proportional elapsed time;
explicit start/end/duration values preserve timing and overlap information. Very
long sessions render all tokens, as the session log already does for utterances.
A future performance change must retain access to the entire recording.

Validation: all 20 dashboard tests passed, including regression cases with
utterances at 30 and 125 seconds, chronological ordering, simultaneous A/B onsets,
missing historical tokens, escaping, and zero-duration punctuation. Browser
inspection confirmed that scrolling down reaches all later words, the table
header remains visible, and document width equals viewport width.


## 2026-10-03 — Timed agent speech alongside human ASR

Context: Great Tree session `merrily-amber-harbor` (database session 30)
contains three human transcripts with 16 tokens and three agent messages without
alignment. The token view previously selected only transcripts, hiding the agent
side entirely. Human ASR does not process the server's own generated agent audio.

Decision: request synchronized ElevenLabs character alignment, prefer normalized
spoken characters, and group them into words across audio chunk boundaries.
Represent synthesized PCM and tokens together in `SynthesizedSpeech`; generated
tokens have no recognition confidence. Validate provider timing arrays and final
completion rather than promoting interrupted responses to completed speech.
The provider clock is cumulative across the entire synthesis request. An isolated
live probe generated 38 words in 11 chunks (11,052.708 ms of PCM, alignment endpoint
11,053 ms). Adding each chunk's PCM offset to these times double-counts elapsed
time. Preserve the provider positions exactly once. The credential-free fixture
in `rust-server/tests/fixtures/elevenlabs/` retains character timings and sample
counts from that probe, with no audio, keys, or voice identifiers.

Anchor those positions to the game clock immediately before server audio
publication. After successful publication, append `conversation_speech_timing`
linked to the original message ID and attributed to its agent participant. This
preserves original text-commit order and lets the dashboard and corpus project
speech timing onto one existing conversation message. Publication time estimates
playback onset; network and browser buffering remain unmeasured delay. Publication
failures do not create a completed speech-timing record. Existing event storage
supports this representation without a database schema change or conversion.

Include agent conversation bundles in the vertical token table. Historical agent
messages remain visible with a timings-unavailable notice; no boundaries are
invented from text. The three old B messages in the named session cannot recover
original word boundaries because neither alignment nor generated audio was stored.
New timings are recorded once the updated runtime is running. Existing providers
without alignment retain audio publication and the speech interval, with no tokens.

Validation: all 239 runtime library tests and one binary test passed, as did four
local agent/client smoke tests, the Speechmatics contract test, and 21 dashboard
tests. Local fake-provider tests check alignment casing, normalized text, cumulative
chunk clocks, split words, and malformed intervals. Integration tests verify
agent attribution and timing in both dashboard bundles and the single corpus
message. Browser inspection using the production renderer showed all three old
B messages beside A's tokens, and 38 aligned B words plus punctuation beside A
in the synthetic recording, with no horizontal overflow. Great Tree also passed `cargo check` against the local runtime. The live
database was read only; the running game was not restarted and no release was
performed.


## 2026-10-03 — Speech controls depend on recorded audio

Context: an agent message can be purely typed, so agent authorship does not prove
that the message was converted to voice. Typed-only sessions should not expose
an utterance/token selector or notices about absent speech timings.

Decision: dashboard bundles carry an explicit `spoken` flag. Human transcripts
qualify directly; agent messages qualify through synthesis timing or a successful
nonempty `tts_publish_completed` record linked by message ID. Existing durable
publication evidence identifies old voiced messages without inventing their
missing word boundaries. This is an event projection, with no database conversion
or compatibility schema branch. Typed and failed-publication agent messages stay
out of the token table. Hide the selector by default and reveal it only when a
speech bundle exists; reset to utterances when switching to a typed-only session,
so a previous token selection cannot hide that session's conversation.

Validation: dashboard tests cover mixed typed/spoken agent replies and session
switching from token view to typed-only conversation. Runtime integration checks
new aligned speech, historical publication without alignment, and an agent message
without publication evidence.


## 2026-10-05 — Integrated clock oracle for generated and recognized speech

Context: existing checks tested provider token parsing and clock mapping separately;
the runtime ASR integration covered a single frame, while the TTS integration used
a mock publisher. Those checks could miss a timestamp error between adapters,
real publication, durable conversation records, and dashboard/corpus projection.

Decision: add one credential-free combined integration test in
`rust-server/src/app/tests/speech_timing.rs`. Local protocol peers exercise both
production provider adapters and the production agent publisher in a human/agent
room. ASR receives four known 20 ms frames starting at client capture offset 9000
ms, with 960 ms without samples between the two turns. Provider tokens retain a
continuous 0–80 ms sample clock; the expected game-clock positions independently
require two 40 ms utterances beginning 1000 ms apart. Delay final ASR output so
its receipt time cannot accidentally serve as audio time.

TTS supplies 1600 ms of known PCM in two chunks, with cumulative character
alignment and a word spanning the 300 ms chunk boundary. Delay synthesis after
text commit. Check all 80 relay frames for sequence, timestamp, and exact payload;
check publication onset between the separately observed provider response and
first browser-socket audio receipt. Bound the ASR anchor between client send and
provider input receipt, rather than deriving its oracle from stored timing. Verify
exact offsets, durations, token counts, punctuation points, confidence semantics,
and both A utterances plus B speech in dashboard and corpus projections.

Tradeoff: deterministic local provider output tests transport and clock accuracy,
not acoustic recognition boundaries or audible playback onset. Wall-clock brackets
allow scheduler/network latency without using broad numeric tolerances that could
hide offset errors. No paid provider calls, production database writes, schema
changes, or runtime restarts are needed.

Validation: the combined integration passed; all 240 runtime library tests and
one binary test passed. The final attribution assertions also passed in the
focused integration run. Targeted formatting checks passed and the context graph
was refreshed.


## 2026-10-05 — Preserve session events in token view and pin log controls

Context: token view previously retained only speech, dropping game actions,
logs, setup events, and typed turns. Session log controls shared the event scroll
container and crowded a single row with the title and polling status.

Decision: expand spoken bundles into token rows while preserving every other
visible bundle at its game timestamp. Both views use the same card renderer for
non-speech content, including structured actions, problem reasons, attribution,
and event indexes. Apply the log/setup filters before either rendering path.
Group simultaneous speech tokens only across adjacent speech entries, preserving
event-index order when an action and token share an onset. Older speech without
boundaries retains its existing notice; typed turns remain ordinary messages.

Make the session log a flex column with an independently scrolling, keyboard-
focusable event region. Keep its header outside that region; the table's column
headings can still stick inside it. Arrange title and polling status on one row,
and the labeled view selector and checkbox group on another, allowing wrapping.
Use a bounded event region on narrow layouts so controls remain available there
as well. This changes presentation only; stored clocks and event data are intact.

Validation: all 23 dashboard tests passed, including production-renderer parity
for actions, typed turns, logs, setup problems, completion, and both checkbox
filters. Browser inspection rendered 30 actions and 30 speech turns in both
views; token mode retained 150 tokens and all actions after filtering logs/setup.
Scrolling the token region to its end left the header at exactly the same screen
position, without horizontal overflow. Screenshot proof is stored in the local
visualization directory as `session-log-events-scrolled.png`.

A 560 px browser-frame check also retained the header position after scrolling,
kept the control group aligned, and had document width equal to viewport width.


## 2026-10-05 — Align the session log with summary cards

Context: the unpadded shaded header looked disconnected from the session summary,
and the token-table explanation and repeated polling timestamp added unnecessary
text to an automatically updating view.

Decision: remove the visible table caption and normal polling-status text. Retain
a concise accessible table name and surface actual refresh errors only. Give the
session log the same white card, border, corner radius, and header inset as the
summary; align the table's first-column text with that inset. Use matching smaller
insets on narrow screens. Keep the header outside the scroll region and preserve
all existing event filters, token content, and automatic refresh behavior.

Validation: all 23 dashboard tests passed. Browser geometry checks confirmed
that the summary title, log title, and view control share the same left edge;
no caption or polling text remains, there is no horizontal overflow, and the
header position stays unchanged after scrolling. Screenshot: `session-log-clean-header.png`.


## 2026-10-05 — One timeline for utterances and tokens

Context: separate table and conversation layouts made changing speech resolution
change speaker placement and event presentation as well.

Decision: one sessionTimeline renderer projects bundles into chronological entries,
expanding only recorded speech in token mode. Both modes use the same timestamp
column, speaker bubbles, non-message cards, filters, and scrolling container.
A bubbles align with the content area's left edge; B bubbles align with its right
edge. Adjacent messages at identical onset share a timestamped row. Token focus
and hover retain parent text and confidence; older recordings retain their whole
message with an unavailable-timings notice. Removed the superseded token table,
separate ordering function, duplicate speech renderer, and nested event timestamps.

Tradeoff: token expansion can interleave events inside an utterance and increases
vertical length; this is the intended finer temporal resolution. Identical onsets
share one row, while event-index ordering remains deterministic. No timing data
or clock conversion changed.

Validation: 24 dashboard Node tests pass, including shared layout, simultaneous
speakers, escaping, historical data, typed-only selection, and event/filter parity.
Browser previews using production CSS and rendering functions verify both speaker
edges, fixed controls through the last token, and no horizontal overflow on desktop
and a 560-pixel-wide frame. Running game binaries still require rebuilding to embed
updated dashboard assets.


## 2026-10-05 — Compact action summaries and actor gutter

Context: full action grids interrupted dialogue scanning; actor identity was
repeated inside bubbles while logs resembled full operational event cards.

Decision: each timeline entry carries a shared role badge beside its timestamp,
using the existing blue A and amber B colors. Simultaneous speech retains one
time row with a badge for each entry. Removed repeated actors and event IDs from
bubble metadata; IDs remain in hover context. Actions use a native details
summary with the type in bold and comma-separated key/value parameters. Matching
player fields remain omitted because the gutter identifies the actor. Steps,
IDs, original payload and additional text remain available through expansion.
Logs show escaped muted text aligned to A's left or B's right edge without title
labels. Unassigned events retain SYS in the gutter. Problem badges and reasons
remain visible when action details are closed; error logs retain emphasis.

Tradeoffs: ordinary lifecycle details take an expansion click; compact action
summaries wrap at narrow widths rather than truncate. This changes presentation
only and does not infer causal relationships from temporal proximity.

Validation: all 25 dashboard Node tests pass, covering inline action parameters,
actor badges, log label removal, escaping, visible problems, and existing
speech/event/filter behavior. Production-renderer browser previews show compact
SetFlow summaries, B logs aligned to the same right edge as speech, expandable
steps/payload, and no horizontal overflow in a 560-pixel-wide frame. The running
Great Tree binary was not restarted; rebuilding loads embedded dashboard assets.


## 2026-10-05 — Plain action summaries and consistent log typography

Context: the disclosure triangle implied additional interaction although the
inline action already conveyed the information needed to read the timeline.
Smaller log text introduced unnecessary typographic variation.

Decision: remove action/setup disclosure controls and the hidden details markup,
retaining compact inline summaries, actor badges and visible problem reasons.
Event IDs remain hover metadata. Logs inherit the normal entry font size and
retain gray text and speaker alignment. Removed obsolete disclosure styles.
Tradeoff: the dashboard timeline no longer displays raw action payloads or
lifecycle steps; durable records and corpus exports are unaffected.


## 2026-10-05 — More space after timeline actors

Context: the 10-pixel gap after actor badges was cramped for plain action rows.
Decision: use one 24-pixel actor-to-content gap for every entry, reduced to
16 pixels on narrow screens to retain reading width. Bubble padding no longer
needs to compensate for an undersized shared gutter. No behavior changed.


## 2026-10-05 — Symmetric timeline gutters

Context: fixed timestamp widths left unused space beside the timestamp, making
the visual gap to the actor larger than its CSS gap. Increasing only the gap
after the actor did not establish a balanced layout.
Decision: the timeline owns a content-sized timestamp column shared by subgrid
rows. Timestamps align right so their visible ends stay aligned even as minute
counts grow. Both timestamp-to-actor and actor-to-content use one inherited
--timeline-gap (24 pixels, 16 on narrow screens). Empty states span both columns.
This replaces fixed timestamp widths and independently configured gaps.


## 2026-10-05 — Align plain logs with plain event text

Context: log messages retained bubble-like horizontal padding despite having no
bubble background, adding an extra indentation after the actor gutter.
Decision: remove horizontal log padding at both desktop and narrow widths.
System and A log text now begin at the same content edge as action summaries;
B log text ends at the shared right edge. Vertical spacing and actor gutter
sizes remain unchanged.


## 2026-10-05 — Prepare coordinated release 0.5.0

The maintainer explicitly selected and requested publication of 0.5.0. Both
registries expose 0.4.3 and neither contains the target. This release records
human/agent speech tokens and improves the shared dashboard chronology and
provider readiness. Supported game/agent APIs and schema 16 remain unchanged
from 0.4.3; no downstream migration guide is required. Earlier-schema databases
must already be converted explicitly. Consumer npm lockfiles are deferred until
the immutable 0.5.0 registry package exists. Publication requires a committed,
clean candidate; Git commands remain separately subject to user authorization.


0.5.0 candidate validation: metadata, make test, 25 dashboard tests, release
builds, Hugo, local packaging and dirty-source publication dry runs all passed.
The first E2E attempt hit sandbox-only Chromium launch denial; the unsandboxed
rerun passed all 13 browser and 35 Prolific scenarios. Package contents were
reviewed (49 Rust files, 50 npm files), with no private configuration or notes.
Cargo cache and unused-local-patch warnings do not affect verified artifacts.
Clean-source dry run requires a committed candidate; npm identity returned E401.
No packages were uploaded and no Git commands were run. The concrete review,
commit, authentication and post-publication checklist is notes/release-0.5.0.md.


0.5.0 publication completed after the maintainer committed the candidate and
renewed npm authentication. Cargo's clean-source dry run passed. Rust and npm
published successfully; exact versions and npm latest were verified from their
registries. npm visibility lagged the successful upload by several minutes;
the original process was preserved and no duplicate publication was attempted.
Clean Rust and TypeScript registry consumers build. Both game-client npm locks
were refreshed to the immutable 0.5.0 tarball and sha512 integrity, and the
--published metadata audit passed. Rust artifact provenance is
 a8c0eb5690e7fcd5009a04ce116a8a0086e124e8 from packaged VCS metadata. No
publishable package source changed after upload. The lockfile completion commit
and annotated v0.5.0 tag/push remain maintainer-owned; no Git commands were run.


## 2026-10-21 — Deterministic session replay and narrow game surfaces

Context: administrators need to launch a replay from a recorded human–human,
human–agent, or agent–agent session. Recording every authoritative state would
duplicate data that deterministic game mechanics can derive. The previous React
`GameSession` also mixed game rendering with connection, conversation, voice,
and lifecycle controls, which made it unsuitable as a shared live/replay surface.

Decision: the `session_created` event stores the one serialized initial
authoritative state and the game's state, action, and rules compatibility
identifiers. Accepted-action events store the actor and action but no resulting
state. The administrator replay endpoint checks those identifiers, deserializes
the initial state, and calls the normal `Game::apply_action` in event order. It
derives both role-specific observations, available actions, completion, and the
conversation timeline. Package release versions do not participate in replay
compatibility. `Game::State` is now deserializable, and games may keep the three
compatibility constants stable across releases that preserve representation and
transition meaning.

The React renderer now receives `GameSnapshot`, containing only the role,
observation, latest accepted transition, available actions, interaction status,
and `act`. Parlando renders conversation, connection, microphone, leave, and
replay controls around that surface. The dashboard opens the experiment's normal
frontend with an administrator replay URL; `ParticipantApp` bypasses participant
registration, consent, matchmaking, and terminal flow, fetches reconstructed
frames, and supplies them to the same renderer. Replay surfaces are inert, while
live game handlers may update ordinary local React state before calling `act`, so
the API does not impose a server round trip before immediate visual feedback.

Tradeoffs: the default replay reproduces authoritative observations, accepted
actions, and messages rather than undocumented transient animation frames. A
future optional display-event extension can reproduce those animations without
changing the stored replay model. Sessions recorded before this change return a
clear replay-unavailable conflict because they have no initial-state record or
compatibility identifiers. No database schema migration is required; the
existing event state column now holds only the initial state.

Validation: the server integration test records an initial state, conversation
message, and accepted action, verifies that only the creation event has a stored
state, and reconstructs three frames for both roles. React tests cover event
jumps, role switching, disabled interaction, and replayed conversation. Runtime,
package-surface, example-game, and browser-fixture builds exercise the narrowed
API and Parlando-owned session controls.


## 2026-10-28 — Agent-agent games appear as ordinary dashboard sessions

Context: the standalone agent experiment runner executes replayable games but
currently writes only runner results and optional traces. Administrators need
to inspect those games through the normal session detail and replay surfaces,
without exposing runner batches, plans, training phases, or controls in the
dashboard.

Decision: when explicitly configured with an experiment store destination, the
runner projects each terminated headless game into the existing participant,
session, and event schema. `SessionRunner` remains the sole owner of execution;
the dashboard record is an atomic completed-session projection independent of
trace capture. A deterministic opaque public session identifier derived from
`(run_id, plan_id)` makes resume idempotent without a schema change, while the
normal random dialogue identifier remains the administrator-facing label. The
dashboard continues to expose only ordinary sessions, details, exports, and
replay. Runner orchestration remains in YAML and runner artifacts.

Tradeoffs: sessions appear only after their headless task terminates, and the
dashboard cannot launch, monitor, or cancel runner work. Runner limits,
quiescence, and cancellation are recorded as a runner-specific terminal event
rather than being misrepresented as human participant outcomes. The detailed
contract, invariants, validation plan, and deferred work are recorded in
`notes/agent-agent-dashboard-session-design.md`.


## 2026-10-28 — Clembench evaluation reads typed completion scores from session databases

Context: an initial Clembench-porting discussion introduced a corpus export,
a parallel game-end taxonomy, and a game-specific post-hoc scorer. Those
mechanisms duplicate existing Parlando responsibilities. `Game::Completion` is
already the structured terminal result and is stored in `SessionEnd`; the
headless runner already has deterministic plans, typed game execution, and a
design for projecting terminated sessions into the ordinary experiment
database. The LM Playschool shared task accepted submitted model systems, which
organizers executed themselves, rather than treating author-produced game runs
as authoritative submissions.

Decision: scored games use a reusable typed completion envelope with a stable
serialized score field, validated binary or normalized numeric score, and
game-specific terminal details. Clembench ports compose that general envelope
with a benchmark-specific typed request-count result. The existing completion
is the only game-score authority. Protocol-aborted games have no quality score;
normal losses have a score, and infrastructure failures continue to use the
existing `SessionEndCause` taxonomy. Headless evaluation sessions are recorded
in one or more ordinary Parlando databases. A read-only evaluator unions those
databases, rejects duplicate plan identities, reads the common completion
envelope, and applies pinned Clembench/shared-task aggregation. Corpus export is
reserved for optional publication after evaluation.

Tradeoffs and risks: consistent score serialization constrains benchmark game
completion types but eliminates a second scorer and permits cross-game database
queries without knowing each result type. Moving source scorer calculations
into authoritative state and completion requires differential tests against
the pinned Clembench game master. Accepted actions are visible to both Parlando
roles, so every port must verify that a raw response is public before using it
as an action; games with private submissions need a safe representation or a
future runtime capability. The detailed port mapping, experiment model,
database boundary, implementation sequence, and provisional difficulty
assessment are recorded in
`notes/clembench-porting-and-evaluation-design.md`.

## 2026-10-30 — Unified actions and native Rust games behind OpenEnv

Context: the message/action split bypasses game transitions for conversation,
restricts training capture, and makes raw-response benchmark ports awkward.
Automatic disclosure of accepted actions also conflicts with private player
submissions. OpenEnv needs externally stepped episodes, while Clembench source
compatibility would introduce Python game hosting and suspension machinery.

Proposed direction: use `Action<Operation, Message>` with distinct `Message`
and `Operation` variants, both interpreted by the native Rust game. Supply a
standard multimodal message representation while allowing custom payloads.
Games emit recipient-scoped delivery effects; participant presentation and TTS
consume those effects rather than raw submitted actions. Agents remain unaware
of audio. A resumable session combines a Rust game with a frozen-policy partner
and exposes the remaining role through an OpenEnv adapter. Clembench games are
ported to Rust and validated for behavioral fidelity; unchanged Python game
masters are outside the compatibility target.

Tradeoffs: Rust ports require manual translation and upstream parity checks,
but avoid a second game implementation runtime. OpenEnv trainers can retain
their learning machinery, while environment-specific agents and scripts may
still need schema, prompt, or wrapper changes. Generic message payloads require
conversion into standard delivery content before generic rendering or TTS.

Follow-up questions: finalize delivery/history semantics, multimodal asset
access, scheduling and yields, announcement sender identity, the Rust/Python
transport, and a pinned external compatibility matrix. This records a design
proposal only; no APIs, databases, releases, or packages were changed. The
current proposal and migration experiences are detailed in
`notes/actions-and-openenv-design.md`. It supersedes the earlier note's message
bypass and public-action assumptions while retaining its completion-score and
evaluation direction.

## 2026-11-19 — Internal ownership boundaries

The live runtime, data projection, clients, and headless runner had independently evolving representations of session state. The implementation keeps the game and agent author contracts at their existing public entry points. SQLite retains its current schema, and the protocol and corpus retain their existing versions.

### Live session ownership, ordering, and admission

A live room owns the mutex that orders admission of its second seat, game actions, conversation, expiry, and connection lifecycle updates. The deadline sweep and socket grace timer recheck the current deadline while holding this mutex. A reconnect timer identifies the exact deadline it scheduled. Outbound actions carry immutable observations and affordances captured from that revision before persistence. The synchronous broadcast and mailbox publication follow SQLite's winning commit under the same ordering boundary. The SQL action writer explicitly reports an already-ended session, and a losing action publishes nothing. The implementation keeps role-private state and the existing participant wire format.

The single optional software seat owns a bounded inbox in its room. The runtime reserves capacity before committing input and prioritizes already queued callbacks before requesting another decision. A full or unexpectedly closed inbox ends that room with a technical failure before the input commits. This trades continued interaction with an unresponsive agent for bounded memory and complete, ordered delivery of accepted transitions. Callback timeouts and shutdown still bound individual agent operations. Ordinary rooms never wait for another room's agent to consume a queue. A terminal commit closes the room's sender and lets the receiver drain, including its terminal observation and finish callback for completed games.

Admission rechecks participant assignment while holding the experiment admission guard. Construction still obtains a durable session ID and logger before invoking the game factory. Initial membership and the initialization marker commit together before publication in live memory. Existing-room memberships also persist before insertion in memory. Initialization errors drain the provisional writer and terminate the durable reservation via one cleanup path. An admitted request runs to completion even if its HTTP client disconnects during persistence. Process failure remains subject to the existing interrupted-session policy.

Regression cases use real HTTP, SQLite, and WebSocket paths for repeated concurrent joins, failed membership insertion followed by retry, a competing durable terminal writer, both roles' concurrent action streams against replay, agent mailbox exhaustion alongside an independent healthy room, and two disconnects within overlapping grace periods.

Only human game connections and their grace periods keep an ended room in memory. A participant who never opened a socket receives the configured grace period from the terminal commit. Farewell audio closes on its own deadline. After room eviction the authenticated participant can still reconcile the terminal result from SQLite.

### Corpus projection and administrator reads

The durable store records session lifecycle, structured terminal cause, and end time, while corpus v1 requires a status string and gameplay-relative duration. A private typed session record now feeds both the single-session dashboard query and the experiment-wide snapshot. A dedicated corpus module owns the v1 projection and validates consistent lifecycle, terminal cause, and timestamps. A running session maps to `playing`, normal game completion to `completed`, participant departure to `abandoned`, timeout or partner unavailability to `expired`, and infrastructure failure to `failed`. An ongoing session has null duration. No persisted values or database schema change.

The corpus game clock begins at game start. Setup logs with a negative game-relative timestamp are retained in SQLite and visible in session history. They are omitted from the v1 corpus, which requires nonnegative event times. A negative action or message timestamp still signals an integrity error. Event indices retain their durable values, including gaps from omitted records.

The store uses one SQLite read transaction for all export tables. The projector groups events and memberships once by session and preserves SQL event and role ordering. Snapshot materialization still consumes memory proportional to the selected experiment. A long export holds a read snapshot while concurrently committed changes wait for the next export to appear. Administrator session detail and participant identity resolution use targeted reads, with experiment membership enforced in SQL.

Regression coverage checks SQLite-backed corpus responses against the downloadable JSON Schema for active, completed and timed-out sessions. Setup log coverage checks the original entries in storage and the subsequent corpus. Schema validation is a development dependency with remote and file schema resolution disabled.

### Participant connection, snapshot decoding, and dashboard selection

The same participant state can arrive in a socket stream, an HTTP reconciliation, or a completed leave request. Keeping React state setters, a mutable mirror, and independent retry callbacks made response order determine which transport and observation the user saw. A private connection controller now owns the socket, heartbeat, retry window, pending ticket, and current view. Each attempt has a generation. Closing or replacing a session invalidates work started by the prior attempt. The latest view gates actions and chat, including callbacks retained by game renderers. Terminal snapshots share the same teardown and farewell-voice decision. Consecutive socket snapshots enforce lifecycle adjacency. Initial and resynchronized snapshots may skip intermediate lifecycle messages. HTTP and WebSocket decode the same platform-owned fields while game payloads stay opaque. This introduces a small transport controller and leaves the public TypeScript package entry points and game props intact.

Dashboard catalogue, experiment, and session reads have distinct ownership. Changing selection invalidates pending responses, clears old detail/configuration from the view, and scopes subsequent requests to the selected resource. Identical reads in flight are shared, which prevents repeated polling from indefinitely superseding a slow response. In-flight requests may finish on the server after selection changes. Their replies and errors have no effect on the new view. An intercepted-network browser test exercises reversed experiment-list, session-detail, and event-poll responses through the production dashboard.

### Installation and experiment resources

The dashboard and experiment routes use separate construction paths. The installation owns storage, administrator authentication, settings, telemetry, and the mount registry. Dashboard handlers use a configuration context that can operate without a live session runtime. Each mounted experiment binds its own typed game configuration and session resources. Mounts use the same definitions for scoped administrative operations on both supported experiment URL prefixes.

Closing intake or saving configuration retires the current admission owner. Existing rooms keep their original game settings, transports, reconnect deadlines, and providers. Reopened intake reads the latest stored configuration. A verified repeat Prolific submission rejoins its existing room. An unassigned direct registration can enter a reopened direct study after giving fresh consent under the current study purpose. Changing recruitment to Prolific requires a fresh verified launch. The HTTP credential format and the single-use WebSocket ticket contract stay the same.

The host retains one small identity and recovery record per successfully mounted catalogue experiment for the life of the process. A full retired runtime remains while it owns a room within its normal lifecycle or transport grace window, or an unused registration within its configured waiting timeout. Periodic cleanup releases empty owners. Terminal participant reads then use SQLite. Sixty-four lock slots serialize configuration changes and construction. Unrelated names can briefly contend on a slot, and unknown namespaces allocate no permanent entries. Opening the first mount reconciles all unfinished SQLite sessions from the previous process. Replacing a mount in the same process preserves every live session.

Admission and live-resource accounting include all retained revisions of the same experiment. Only the current revision contributes configured limits to installation load. Verified participant credential generation and cleanup are scoped to the issuing owner. Revoking an expired registration on an older revision cannot revoke a newer credential for the same durable participant.

### Headless accounting and resumption

Only the current training update owns accumulated trajectory steps. Run-wide accounting retains scalar counts and the durable result files retain complete per-session trajectories. Resumption reads the run ID, plan ID, status and learner steps together from each result exactly once. Fresh and resumed work use the same status classification. This keeps previous checkpoints out of memory while preserving plan identity, result filenames and the learner's existing idempotent update contract. Memory still scales with the trajectories in one pending update.

### Remote Python agent lifetime

An instance record binds each Python agent to its logger and callback lock. All callbacks for that instance drain their own logs under the lock. Shutdown shares the same lock, waits for earlier callbacks, and releases the registration after cleanup. Distinct instances can run independently. Creation reserves capacity on the asyncio service loop before awaiting the user factory. A stalled callback can delay shutdown for its own instance until it finishes or is cancelled.

### Isolated HTTP fixtures

Each server fixture owns its issued credentials. Anonymous, participant, and administrator requests send one caller-selected payload and observe the first response. This requires test callers to supply identity explicitly, makes authorization failures visible, and prevents unrelated concurrent tests from supplying substitute credentials.

## 2026-11-19 — Offline code atlas

The architecture guide in docs/code-atlas opens directly from a checkout. Hash routes address a component, evidence view, or action step. The map focuses on the five boundaries of live play. Its connections represent interactions, not a complete dependency graph. Source excerpts are embedded so opening the file requires neither a server nor permission to fetch local files. Run python3 scripts/refresh-code-atlas.py when the implementation changes and review the accompanying explanations. Recorded test passes refer to the dated local run and must be reassessed when fixtures or behavior change.

## 2026-11-19 — Source-backed maintainer reference

The replacement for the five-component overview lives in developer/code-atlas, outside the user manual and private working notes. Its directed neighborhoods describe witnessed runtime relationships, including calls, transport, channels and durable reads. Their edge counts would not measure static fan-in or fan-out. The session explorer keeps room lifecycle, pause, participant projection and retention distinct. Flow steps identify where a write becomes durable and where publication can occur. A searchable offline source bundle contains complete Graft symbol spans. Regenerate it with python3 scripts/refresh-code-atlas.py and review the editorial edge witnesses against changed implementations. The dated local result covers the Rust library suite (252 passed); the separate browser matrix is listed as inspected. The atlas does not run tests in the reader's browser.

## 2026-11-19 — Orient the internals atlas around a concrete room

The atlas opens with the Cue Choice Dax fixture to explain the distinction between room state, game mechanics and a role’s view. Each of the fifteen responsibility guides names inputs, outputs, motivation and an ordered list of source definitions. Paths and line anchors are derived from the existing offline source bundle. A reverse file index makes responsibilities implemented in the same module visible together. Dependency edges now lead to related reading paths, and regression fixtures can support more than one subsystem. The competing-deadline illustration models a single server evaluation for a paused running room; its example timestamps are independent of deployment settings. A real-socket regression in rust-server checks that reconnecting after idle but before grace returns the same room’s idle ending. The fixture permits either a prior cleanup sweep or the reconnect check to commit the terminal outcome, matching the externally observable contract.

## 2026-11-19 — Developer reference as technical documentation

The overview defines game, experiment, session, and observation in dependency order, then uses one existing Cue Choice fixture to relate the definitions to the code. The subsystem and file indexes hold lookup material on separate routes so the initial reading path can remain short. Documentation links retain the offline symbol bundle and line anchors. The dependency diagram documents selected calls, and the deadline illustration explicitly assumes one evaluation time; neither is a complete execution model.

## 2026-11-19 — Reading order and source boundaries in the developer reference

The overview introduces the players and their task before the Cue Choice state machine. It opens before the deal and treats a wrong-role request as an alternative to the first move. The view labels paraphrase the observations. The adjacent source controls retain the precise enum variants. The dependency map describes the operation at each edge and identifies entry definitions for responsibilities that span or share files. The file index uses the shared move handler to explain both a possible module split and the synchronization constraint such a split would need to preserve. These are editorial groupings; a code change to a role, observation, or handoff requires checking its corresponding explanation.

## 2026-11-19 — Functional units and mutation boundaries

The live server, SQLite store, experiment runner, and participant shell each put independent operations in a single implementation file. We split them behind the existing game, agent, runner, and client interfaces. File size prompted review. Ownership of an invariant or a separately testable computation determined the boundary.

* A live session owns its mechanics state, lifecycle, pause, and participant readiness. The connection registry owns socket generations. Admission carries a reservation while selecting or constructing a room. The action coordinator keeps expiration, queue reservation, durable commit, state installation, and delivery in one ordered operation. Network handlers and agent callbacks use that operation. Changing the lock scope or publishing before the durable write would alter the concurrency contract.
* Privacy reporting reads a scoped collection of configurations and builds one report consumed by the HTML, Markdown, and JSON routes. Export schema ownership stays with the corpus projector. Direct tests found that report text had been used as an identity key, causing repeated synthesis-service rows in an installation with multiple experiments. Separate sets of provider identities for transcription and synthesis preserve first-seen order and keep distinct data flows visible. Timeline tests now exercise the projection and grouping of durable events inside the owning unit.
* SQLite uses the existing pool, records, and schema. The private modules group operations that change together. The event batch and terminal result remain in one transaction. A failed second insert must roll back the entire batch and leave the first sequence number available to a retry.
* The headless runner keeps its public registry and uses private units for specification, planning, execution, training, and artifacts. The artifact store binds a run ID to its directory, verifies plan and checkpoint identity on resume, and publishes finalized records atomically. These invariants can be exercised without starting a game.
* The browser shell coordinates the participant connection and rendering. Voice preparation owns its asynchronous cancellation and device lifecycle. Session controls own the chat draft during a pause. Integration suites continue to exercise the real public interfaces alongside the local tests.
* The main tradeoff is additional navigation across explicit boundaries. The offline code atlas records the new symbol locations. A change to session serialization, a new retention rule, or another public entry point should be checked against its corresponding contract tests and atlas examples.

## 2026-11-19 — Contract audit after the functional-unit split

The source comments and the functional-unit audit in this directory describe the observable behavior at the new boundaries. Tests exercise the public caller when the invariant spans several units and the owning unit directly when ordering, rollback or time must be controlled.

Participant deletion holds a SQLite write reservation before computing the affected graph. The live-room check, session list and deletes now share one snapshot. The read-only preview also uses one read transaction. This holds the writer during the preview queries, in exchange for ruling out a stale session list within the commit. The dashboard names the shared sessions before confirmation and refreshes the list after a successful delete. Administrator activity uses a monotone timestamp so an earlier request finishing late cannot shorten a valid session. No schema conversion is involved.

Checkpoint parsing uses the same nonblank, opaque identity rule as construction. Resumption checks the persisted update ID and base alongside the run ID and checkpoint number. Valid files keep their wire format. A mismatched record stops the run because applying another policy's update would misattribute subsequent trajectories.

Live expiry and the dashboard share their deadline candidates. The next scheduled deadline is chronological. Evaluation after several deadlines have elapsed uses cause precedence. Socket retirement drops registrations by complete session ID. Tests cover distinct roles, prefix collisions and deadlines expressed in different time zones.

A replay URL owns its React component lifetime. Requests that finish after a switch cannot restore the preceding error or frames. Microphone device scans publish only the latest request's list, and a denied passive scan preserves the last list. The chat form checks interaction state on submission so a retained draft survives a pause.

## 2026-11-19 — Source ownership of functional-unit contracts

Module introductions describe the unit’s state or computation, its immediate callers and dependencies, and the guarantees that cross that boundary. Existing function comments retain the detailed ordering, failure, and retry rules beside the implementation that enforces them. The contract audit remains the index of behavioral tests. Moving an implementation requires carrying its contract to the new owner and refreshing source spans in the maintainer atlas.

## 2026-11-19 — The Last Screening

The game needs repeatable staging problems and a recognizable theater shared by two operators. Rust evaluates an ordered score of optical predicates on discrete lamp and flat detents. The score separates conditions that must stay true from the next required picture. Each seeded reel starts off its target marks. The projectionist holds the tower cue and the stagehand holds the aperture cue. New compositions can reuse the same predicates, slides, scene geometry, and wire actions. A reel with a new ID also needs a participant cue illustration and instructions.

Babylon.js renders the two rooms from the public stage picture and the equipment owned by the seat. Clickable machines move the first-person camera to inspection positions. Local animation handles fly-rail travel, occlusion, projection shafts and the safety curtain. The server evaluates settled equipment states so frame rate and network timing do not determine success. The single-seat Vite preview supplies example observations only in development. Live play and completion run through Parlando.

The launcher defaults to the sibling Parlando packages. Both registry manifests specify 0.5.0. Cargo's configuration is discovered from its invocation directory, so published builds run outside the checkout and local builds supply an explicit path patch. npm installs the locked dependencies before local mode substitutes a link to the sibling SDK. The published browser SDK and the checkout have different participant render callbacks under the same version number. A small adapter handles the action callback and the registry SDK's intercom controls. Parlando continues to own transport, admission and outcomes in both modes. The Rust game uses the shared trait surface and the checkout's initial replay-version defaults.

The built-in image generator supplied the midnight seascape, repeating Deco wall panel and lighthouse cutout in client/web/src/art. The scenery directions were a hand-painted Scottish sea without moon, vessel or lighthouse; a face-on walnut and patinated bronze cinema wall bay; and an isolated Bell Rock stone lighthouse on transparent canvas. The moon and ship are separate projected slides. Both camera views, the two-reel playthrough including role chat and exits, and 256 seeded solves were checked during implementation.

## 2026-11-19 — Solo operator in The Last Screening

Human–agent pairing puts the human in seat A. An optional game setting assigns the projection booth to either seat, so the same registered partner can operate either room. The standard assignment remains A in existing games and replays. The operator reads its role-specific observation, exchanges its private one-based cue mark in chat, and chooses from the ordinary legal-action catalogue. Receiving a new reel clears the earlier mark. This keeps the clue exchange playable by one person while preserving the information split used in human–human sessions. The policy knows the two shipped reels; new reels with different lantern choreography need corresponding behavior for a computer projectionist.

The Last Screening launcher prints the experiment dashboard URL immediately before starting the server. One resolved port supplies both the URL and the listener argument. The dashboard works with every experiment database and provides each experiment’s participant link.

## 2026-11-19 — Counterweight Hotel

A single East cable coordinate determines both suites’ positions. Parked guests can exchange luggage with staffed landing racks. Finite car capacity supplies the balance puzzle, while replenishable racks make wrong turns recoverable. Travel is quantized to a one-floor release and a separate brake action so network delay cannot decide a puzzle. Route data specifies initial and exit ranges and an optional pair of delivery manifests. The second itinerary uses the same machinery but requires a reversal at the exchange grille and different final loads. The physical controls reveal neither a valid seal nor destination through the list of available exit attempts.

The porter acts through public observations and intercom messages. It can share a dispatch and operate independently, but needs the guest’s ledger entry to complete its own checkout. A rejected lock attempt waits for a corrected entry. Human and solo experiences use the same server and database. The new local demonstration uses text communication with its own dashboard credentials stored under the ignored, user-only .local directory.

Babylon.js interpolates accepted floor changes against the shaft, opposite car and Alpine vista. The scene represents luggage counts and removable brake ownership. A keyboard-accessible dock offers the same actions as picking props. The React participant boundary handles the checkout and published 0.5.0 SDK callback differences used by Last Screening. The built-in image generator supplied five local textures: morning Alps, Jugendstil wallpaper, a marble landing, a woven carpet, and a travel trunk face.

The launcher selects local or published Parlando as a pair. The browser build stamp covers browser sources, selected local SDK sources, dependency manifests, tool versions and output assets. An ordinary restart reuses the verified bundle and lets Cargo do its incremental check. Changing modes can rewrite the game's Cargo lock resolution; the delivered checkout is in local mode.

### Walking inside the Counterweight Hotel suites

The two suites now enclose a walkable space on all four sides. A local camera tracks the guest within their car while the shared cable state drives the view outside. Local movement needs no server events or changes to recorded puzzle actions. A circular walking footprint slides along walls and furnishings. The exchange grille stays a boundary at every floor. New furniture that blocks a walkway needs a corresponding footprint.

A writing desk at the rear of each suite opens the same guest ledger as the keyboard-accessible controls. This gives the guest a reason to turn away from the shaft while preserving both itineraries, the porter policy, and access to every action from the controls. Keyboard and touch movement pause when a dialog takes focus.

### Counterweight Hotel exterior and environmental clues

The window bay and lift well have separate geometry. Deep reveals mask the edges of the alpine panorama. Continuous masonry encloses the five-storey shaft. The moving counterpart has an opaque shell and a fluted opal pane above a blind transfer hatch. The relative floor animation moves the entire shell and hatch together. The thin invisible interaction meshes remain independent of the visible surfaces.

A dated workshop plate records the paired-car mechanism in the hotel's own visual language. The concierge's coloured reservation copy derives only from the current participant's view, including any luggage count. The stamped reservation panel and accessible control names provide the same information without requiring close visual inspection. The two suites preserve different halves of a rescue story, linked by torn rope and the porter's striped cloth. These objects add a reason for the guests to compare their rooms without adding another puzzle dependency. The porter asks for reciprocal booking information while leaving mechanical deductions to the guests.

### Making the hotel service legible from the opening

The guest’s objective and first social action appear together on a front desk slip. It follows the actual rendezvous, handoff and checkout states, using only each guest’s observation. The ledger links to the participant intercom so the guests still exchange their own bookings. Both luggage counts and their relative pull appear at the rack as cases move. Guests can inspect the hatch, brake and landing lock when an operation is unavailable. The dock and physical stations share the same panels and only the server catalogue can authorize a game action. The hatch has a separate pick surface in front of the grille. The commissioning plate and rescue keepsakes support inspection, while the window and incidental furniture remain scenery.

### Counterweight Hotel interaction outlines

Each usable furnishing has a persistent turquoise silhouette while the room accepts input. Babylon’s depth-aware selection pass groups visible mesh parts by the same interaction ID used for picking. Transparent hit areas stay out of that pass. Rescue gear uses a shared hit area and individually marked visible parts. The short prompt uses the same colour sample and changes from a general click or tap instruction to the action under the mouse. The outline pass pauses while an instrument panel is open.

### Hover-only hotel affordances

The selection layer starts empty. Changing the hovered object selects only its visible parts and updates its action prompt. Mouse leave, drag and instrument panels clear the selection. Cases, the luggage rack, the reservation copy and the ledger can share controls while keeping independent visual identities. Touch taps use the same pick path without requiring a prior hover.

### Account for baggage on the landings

The earlier model kept only the two car loads. Each wing now keeps a five-floor ledger. Each landing starts an itinerary with three cases and has six rack slots. A transfer conserves the wing’s total and changes exactly one rack and one cabin. Local transfers have priority. An empty source or full destination uses the nearest eligible rack on the same wing, breaking ties toward the lower floor. This preserves the original control availability and recovery from detours. Published observations supply the selected floor for each direction. The current wing sees its own racks and both car loads.

The luggage panel frames both racks across a cutout in the cabin wall. A case changes its attachment from the cabin to the stationary shaft, with a visible hoist carrying it through the opening. An inter-floor transfer follows the separate service well. Queued, acknowledged transfers animate in sequence. Opening and closing the rack preserves the guest’s walking position.

The interrupted live session was ended with authorization to update port 8081. Before the transactional replay conversion the hotel process was stopped and the SQLite file checked for open handles. Backup: games/counterweight-hotel/.local/hotel.sqlite.before-baggage-20261119T153714129154Z.backup. Storage schema version 16 was retained. All 16 recorded initial states received two five-rack allocations of three cases and updated state/rules compatibility metadata. Historical landing counts are reconstructible from accepted load/unload actions; they were never individually observed in the earlier game. SQLite integrity, foreign keys, unchanged table counts and unchanged unaffected row contents were verified against the backup, including 16 sessions, 20 participants, 662 events, and 31 session-participant rows.

After restart, the live administrator replay endpoint reconstructed all 16 historical sessions. All 840 role-specific frames had five bounded landing inventories and conserved the total case count within each itinerary and wing.

### Counterweight Hotel document inspection

The workshop plate and each booking document use a shared Canvas2D drawing in the scene texture and the inspection canvas. The wall copy and desk ledger keep their own layouts, and both record the partner's wing, destination, seal, and any baggage delivery from the local observation. The scene redraws the booking textures only when those values change. The canvas supplies an accessible description because its printed text is rasterized. Only the book and framed documents own document interactions. Ordinary enamel labels remain scenery and the mechanisms retain their own pick targets.

### Counterweight Hotel demo voice

The paired demo uses Parlando's built-in live guest-to-guest audio relay. The dashboard setting changed in paired configuration revision 2 while no sessions were open, and the same experiment was reactivated. Transcription and speech synthesis stay disabled. Participants give browser microphone permission to prepare voice and can still use text. A test on port 8081 with two independent Chromium contexts and synthetic microphones verified that both seats sent audio frames and received nonzero audio from their partner. The test session was ended through participant controls.

The hotel porter's policy consumes conversation text and returns text. Great Tree has separately configured transcription and speech synthesis. The hotel demo has no corresponding provider credentials or approved transcription flow, so spoken porter dialogue is pending while typed dialogue remains available.

The two cars have permanent, unlatched brass ventilators facing the same full-height masonry air well. Their voices carry through the shaft at every stop and in transit. The grilles and their labels are scenery. Communication belongs to the participant session and continues independently of the walking camera, baggage view and document inspection.

### Solo porter booking receipts

The porter used to store a recognized reservation silently when it was waiting for the other guest to act. The revised policy sends a read-back of the floor, seal and optional luggage count, even when the guest repeats the entry. At a parked Floor 2 rendezvous the West porter names the handle handoff as the next action. Replies can accompany a legal physical action in the same agent response. A peer porter’s receipt does not trigger another receipt. The porter identity version distinguishes the new policy in recorded sessions.

### Fixed brake levers and a transferable key

Each Counterweight Hotel lift has a permanently mounted brake lever. One brass key unlocks the lever in the lift that holds it. At Floor 2 the aligned hatches let East pass it to West. Both levers remain visible throughout the exchange so the mechanism matches the room, workshop engraving, and porter’s instructions. The existing pass action and handle observation field already encode this exclusive ownership, preserving game rules and recorded actions.

### Making the Counterweight Hotel cable and delivery observable

The old solo session record shows that the porter reached East’s second booked floor, then emptied its car to satisfy West’s zero-case delivery. The guest continued trying to travel to the first booking. A separate stall occurred if the porter acquired the key before hearing its booking, although the coupled floors already determined both destinations.

The first service now opens with two empty cars and East one stop above the hatch. Loading one case into East produces an observable change from equilibrium to descent. One release advances a single stop under gravity. Parking preserves position and loads. The server supplies each wing’s next possible landing from the same calculation that accepts a release. The client displays both weights, the available direction and the reserved landing. The 3D cars share one interpolated cable position, with West’s or East’s cases visible through the opposing rack and only the keyed lever operated. New services begin at their own initial pose. Stage one’s new initial deal changes new sessions only. Recorded initial states and later-stage deals still apply the same action rules.

The porter derives travel from its partner’s ledger and the fixed sum of floor numbers. It loads or unloads its own car, asks for an explicit case adjustment if the other guest must act, and gives arrival instructions using that guest’s floor, seal and delivery count. Its own seal and ordered load must still come from the other guest. The porter waits 900 ms before parking so individual stops remain visible. A custom agent response timeout must allow time for this pause. It resets its booking on each service and can repeat the current one. The next service announces fresh bookings and the luggage needed for checkout is distinguished from the ballast used for travel.

### One evacuation per hotel session

Both guests reaching the old first exit triggered a fresh hotel booking inside the same session. The earlier destination had no visible significance beyond a desk entry. A session now has one physical objective: reach the two rescue bridges across the avalanche-damaged hotel, unlatch the gates and cross together. The default route teaches the freight balance and key exchange. The extra freight-manifest puzzle is a separately configured rescue-supplies run. Each route retains its seed-specific deal. Crossing both gates completes the session immediately.

Both parked cars must align with their bridges after the key exchange for the arrival signal. The HUD marks the milestone immediately. Once the rendered cabin docks, the bridge replaces the ordinary landing and its lamps illuminate behind the closed gate. A short bell accompanies an actual arrival when browser audio is available. The gate opens after the guest uses the correct seal. The refuge ending appears after both guests cross. Supply alignment and crossing remain distinct so crews can see the bridge while finishing their loads.

State now stores the selected route in place of a progression index and a seed for another service. The local SDK records state and rules version 3. Historical version 2 sessions need the earlier hotel binary for replay. No stored session records were converted. The launcher enables the local-replay Cargo feature only when using the local SDK, since the published 0.5.0 Game trait predates the replay constants.

### Walkable avalanche terraces

The rescue brief used to explain the avalanche before guests saw any damage. Both routes now start facing a terrace doorway. Parked lifts let either guest walk onto a fixed hotel gallery to see the broken stairs, damaged service gallery and refuge across the ravine. The route instructions are available on request. The terrace entrance and the sealed bridge gate sit on different sides of the car so exploring a blocked route cannot accidentally complete the escape.

The server records each guest’s location when they cross the threshold. Walking, touch movement and the accessible dock use the same actions. A guest on the terrace can always board again. Both lifts remain parked until both guests are aboard, including when the porter operates the opposite brake. The walking controller retains the guest’s heading on a physical crossing and restores the correct side after reconnecting. The stairwell, avalanche and distant structures stay in shaft coordinates as the lift moves. Existing replay snapshots place both guests aboard, matching their original rules.

The user authorized restarting the Counterweight Hotel demo to apply future changes, including ending an active game on its local port.

### Hotel puzzle feedback and discovery

The hotel supplies observations through its scenery, illustrated mechanism plate, private ledger, gauges, and inspection panels. Hover identifies the object under the cursor. The arrival bell, lamps, and banner acknowledge a successful docking. The client no longer generates route advice, appends instructions to milestones, or highlights the next control. The solo porter acts on the mechanism and exchanges ledger information as the other guest. It can report what it knows and acknowledge a failed attempt without giving the player a sequence of actions.

### Directional lift sounds

Each guest hears their own lift travel. The sign of the animated cabin displacement selects one of two quiet mechanical loops: a rising sheave whine with fast ratchet clicks for ascent and a lower, descending drum with paired knocks for descent. The render loop starts playback when the car actually moves, including after baggage work has finished, and fades it at the stop or a reversal. A stationary brake stroke and a fresh session snapshot produce no travel cue. The sound is independent of camera direction, voice settings and the bridge arrival bell.

### Counterweight Hotel exterior and cabin motion

A copy of the broken staircase on every deck and opaque service doors made the same terrace appear to follow the guests. The hotel now occupies stationary world coordinates. The guest suite and opposite cabin each translate four metres per floor in opposite directions. The camera, cases and hoist use the frame of their current support. The crossing to a terrace places the guest camera in the building frame. The server continues to interlock the brakes while either guest is outside.

The broken stair flight belongs to the second-floor terrace. Each of the other four landings has a different edge profile, visible through the sliding service-door grilles while the car travels. All five decks share a walkable footprint so crossing and collision behavior are consistent. The bridge and refuge are placed in the building frame at the paired destination elevations selected for the route.

### Keeping the refuge in sight

The bridge previously appeared only on docking and ended at a sign. Each wing now has a permanent access gallery and trussed footbridge at its destination elevation. The gallery turns around the hotel and meets the near bridge joint. The far end meets the refuge’s doorway. Guests can see the span from their terrace throughout play. Only the short gangway across the opposing lift track deploys on docking. The threshold position and rooftop railing preserve sightlines across the possible starting and destination heights.

### Making the solo porter's ballast changes perceptible

In a live solo session, the porter moved two cases within a tenth of a second of the guest setting the brake. Travel itself preserves cargo. Each porter transfer now carries a chat message with the rack used and the resulting cabin load. The runtime delivers the message after accepting that transfer. The porter gives each brake stop and each of its transfers 900 ms to register before another decision. Ledger clues and travelling with a human partner retain their existing rules.

### The bridge inspection lock

The three gate marks previously appeared as names on buttons, with no connection to the bridge's condition. They now represent the individual stamps of the hotel's bridge inspectors. The inspector sets the latch after testing the span and files a stamped copy with the opposite crew for an independent check. The in-world lock, its close-up controls and the paper records share the same die artwork. Docking engages the release mechanism, while the existing server-side seal and supply checks still determine whether a crossing succeeds.

### Crossing the bridge and resetting the lift

Unlocking used to mark the guest as having escaped, which disabled scene input immediately. In rules version 4, unlocking pins the cars to their bridges and releases the gate. Each guest reaches the refuge independently. Manual and assisted walking use the same physical deck and slope coordinates, and the lodge lights come on before the client reports arrival. Both arrivals produce shared completion. A reconnect at an unlocked gate can repeat the crossing, and a guest already marked safe reconnects at the lodge.

An incorrect inspection seal returns the coupled cars to the central safety catch when both guests are aboard with closed gates. If a guest is outside or a gate is open, the local inspection lock trips instead; the brake pedestal rewinds the spring for just over three seconds and resets it without moving the cars. A brake release advances one load-driven detent and catches automatically. Client sound, lever movement and the travel interlock follow the animated position.

A visible suitcase click selects the mesh and transfers it according to its supporting rack. The server checks cabin capacity and the named source rack. The client animates the selected case on acknowledgement and fills its former slot. The accessible dispatch control can use the nearest eligible rack when the local rack is empty or full.

Game URLs, experiment configurations and session records stay in the same local installation. The replay manifest changes from state/rules 3 and actions 1 to state/rules 4 and actions 2. The previous executable and participant build are retained in games/counterweight-hotel/.local/replay-v3 for replaying version 3 sessions against a copy of the database. A replay request for a v3 session on the new server correctly reports a version mismatch. The retained build does not cover earlier v2 sessions.

### Bridge power, freight, and arrival signals

Passing the key at the central exchange now seats it in the West master contact. The commissioning drawing, socket, receiver and peaked gallery entrance carry corresponding physical marks. Both bridge circuits energize when the key seats. The level gauge reports weights and balance, leaving guests to predict travel. The solo porter uses a dispatch assembled from the other guest’s messages before it drives onward, and asks for outstanding facts at the handoff. It does not reconstruct its own route from the opposing bridge elevation.

A freight order pairs a stores floor with the two delivery ranges. Rescue Supplies adds a dedicated green dispensary chest to the East stores rack, one detent beyond the exchange. Only East can collect it at the powered dock with both guests aboard and room for a case. The case is counted separately from ordinary landing stock and occupies one cabin slot for the rest of the run. The service hoist carries the same mesh to the cabin. Both the signed cargo counts and the collected chest govern the freight scale. The inspection lock engages only after docking and freight clearance. Every incorrect engraving trips the local rewindable spring, preserving the solved lift journey and the partner’s position.

Each wing has an arrival lamp on the refuge porch. The local arrival camera acknowledges reaching the threshold before the server ends the game. The completion view shows both lamps lit even when the runtime replaces the scene immediately. The local release and restore paths avoid replaying an already seated key’s relay sound.

This revision uses state version 5, actions 3 and rules 5. The installed version 4 server and corresponding browser bundle were preserved in games/counterweight-hotel/.local/replay-v4 before rebuilding. Experiments and existing recordings retain their identifiers and database schema. Historical v4 replay requires its preserved build and a copy of the database.

### Counterweight Hotel entry view and exterior doorway

The first cabin view looks diagonally across the carpet toward the lounge furniture, windows and brake. Walking back inside still preserves the guest’s heading. Both wings share the initial sight line, with their own furnishing arrangements.

The moving jamb defines the clear service opening. Cabin wall returns and fixed masonry terminate at staggered offsets behind it, and the terrace paving meets the brass sill at its outer edge. Intermediate cornices stop beside the doorway and each lintel ends below the next terrace. These clearances eliminate competing depth surfaces and keep trim from the floor below projecting into the walking route.

### Counterweight Hotel exterior landscape

The window billboard and distant gallery cards each carried independent mountain images, so a guest turning on the bridge could see their edges and the empty space below them. All openings now share one 4096 × 2048 equirectangular alpine photograph on a camera-centred Babylon sphere. The sphere is created after the cabin furniture is grouped so it remains a scene root during lift travel. A separate textured gorge in shaft coordinates provides parallax and a visible channel beneath the span. Vertex opacity tapers the distant perimeter into the photograph. The terrain is scenic and does not participate in walkable-floor queries or picking. Photographic sources and licenses are listed in the game README.

### Counterweight Hotel exterior finish scale and delivery (2026-11-20)

The terrace, gallery and refuge share nine small tiled colour and tangent-space normal map pairs. Their repeat dimensions are in metres, so moving along a long ashlar wall retains the same course height as approaching a short parapet. Box faces use construction coordinates for stone and paint, with local grain axes and slight per-member offsets for timber. Flagstone and transverse bridge planks use actual joints in the geometry. Bridge planks are merged by material within their moving or stationary groups. The walk surface and crossing coordinates remain independent of the small render-only joints.

Map synthesis is an offline, deterministic authoring step in the web client's `tools/bake-exterior-finishes.mjs`. The WebP tiles total 590,008 bytes. Browser startup needs texture decoding and upload, with mipmapping and anisotropic filtering for oblique views. The baked normal maps and tiling trade unique large-scale building weathering for stable close-range detail, fewer uploads and no browser-side noise generation. The rounded avalanche banks share the snow map and retain smooth normals. Compacted ice and slate use faceted normals and distinct relief tiles to preserve visibly steeper fracture faces.

The exterior shell includes the hotel’s side walls, casements, plinth and roof around the walkable gallery. The refuge sits on a continuous ledge whose snow and rock share a rim. Its toe extends below the gorge bed for all booking heights. The bridge has cross-girders and sway braces visible from the lower terraces. A directional light casts shadows across the outdoor architecture. Because its casters are fixed for a given booking, the shadow map refreshes when the booked elevation changes. The completion illustration is a render of this same scene with both arrival lamps on.

### Counterweight Hotel brake interlock and suitcase dialogue

Both cars use one removable brake key so only one operator can release the shared cable. The East socket operates the local brake and the West master additionally powers both bridge gate releases. The two workshop plates show this circuit beside the counterweight diagram, and the gate and socket markings identify the same feed. Suitcase racks and the cable gauge describe the present weight relationship. The porter attaches its motive to each committed transfer and recalls it for follow-up questions. Motives distinguish changing the direction of travel, freeing space for the dispensary suitcase, and meeting a delivery manifest. Participant-facing instruments, documents and dialogue call these objects suitcases. Players can still use older colloquial wording in chat.

### Counterweight Hotel camera comfort

Both lift cars previously began a trip with exponential catch-up, reaching peak velocity immediately. Guided movement also rotated toward each new path segment without a turn-rate limit. The terrace controls repositioned the walking camera between separate collision margins. Pointer drags used a fixed angular gain per pixel, so a full-width drag turned farther on a larger screen.

The participant view now defaults to reduced motion and stores the guest’s override and vertical field of view locally. Audio and transit status accompany a discrete floor change in this mode, and an explicitly requested guided crossing cuts to the porch. Guests can still walk across manually. Animated travel has a rest-to-rest curve and a capped guide turn rate. The camera starts level, walking accelerates into a 1.75 m/s stride, mouse and touch turning scale with the displayed view, and a shared walkable sill keeps the guest’s heading and position across the server-approved transition. Manually arriving at the porch keeps the guest’s own view.

In Chromium, fixed 16 ms scene steps produced only the two endpoint heights during reduced-motion travel. Animated travel covered the same 4 m, with a largest frame step of 0.054 m. In the doorway check the location change advanced the camera 0.004 m to the sill, and releasing movement input left the camera stationary. A browser playthrough verified the default mode, saved field of view, guided crossing and celebration.

### Counterweight Hotel scene rendering and pointer sampling

The entry view previously drew hundreds of individual planks, brass fittings and grille bars. Rigid parts now share a mesh when they belong to the same assembly and have the same opaque material, vertex attributes and interaction identity. Batches use the assembly's local coordinates so a suitcase, door leaf or brake pulley can still move independently. Transparent hit areas remain separate. The canvas renders one pixel per CSS pixel, trading some high-density detail for less GPU work. Hover sampling runs at most twenty times per second, including when scenery moves beneath a stationary pointer. A click still resolves the exact pointer position immediately.

On a 1000 × 700 CSS pixel Chromium viewport at device pixel ratio 2, the warm entry view went from 538 to 258 draws per frame and from 1,143 to 624 scene meshes. The framebuffer fell from 1600 × 1120 to 1000 × 700. A comparison of 2,320 rays from four camera positions produced identical actions and occlusion. In a short SwiftShader run, median and 95th-percentile frame intervals fell from 159 and 218 ms to 141 and 197 ms. Software rasterization limits the applicability of these timings to players' GPUs.

On the same viewport using Chromium's ANGLE Metal renderer on an Apple M5, median and 95th-percentile entry frame intervals were 16.8 and 17.7 ms before the change and 16.7 and 18.4 ms after. Both sustained approximately 60 fps in this short sample. Median and 95th-percentile JavaScript render times fell from 2.7 and 3.3 ms to 2.3 and 3.0 ms.

### Counterweight Hotel observation window, skyline and lift animation

The north masonry wing projected several metres beyond the observation glass and obscured the peaks from inside the cabin. The observation wing, plinth and roof now end at the glazing plane. A shallow window reveal and a splayed shaft partition preserve a solid enclosure around the neighbouring car and open the oblique sightlines. The shared exterior once again uses the original alpenglow artwork, wrapped continuously, fading into glacial haze below the bridge and clear sky above the mountains.

Automatic lift travel and guided crossing are independent camera preferences. Lift rides default to the eased animation even for browsers with the previous reduced-motion preference, which still governs the guided crossing. An explicit choice to skip lift animation is saved separately.

### Counterweight Hotel automatic brake pacing and receipts

The porter waits after observing a committed key pass or brake release. The standard browser animation seats the key after 1,770 ms and travels one floor in 2,250 ms. The porter allows 2,000 ms and 2,500 ms respectively, with a margin for normal frame and delivery timing. These are agent presentation delays rather than new game rules. Slow or suspended browsers can still render the scene behind the server.

Every porter release carries a chat receipt through the existing action-and-message response. The runner sends the receipt after accepting the release. The receipt identifies the operated wing and the purpose of the one-stop stroke, and the resulting floor indicators display where both lifts actually landed. This keeps the dialogue accurate even when another guest changes the balance near the time of the action.

### Counterweight Hotel lift playback under rapid observations

The hotel scene presents acknowledged changes in a single queue. Each cable release reaches its own landing before the next travel cue starts. A brake-key transfer holds both cars at level 2 until the key seats. That frame closes the bridge contact and can play the relay sound before the next stroke. A local hoist trip holds its position in the same sequence, with the existing hoist timing and catch-up rate. The scene reports completed landings to the view so the gauge can follow the presented motion. Initial scene setup installs the recorded floor and seated key immediately.

The playback honors the stationary audio cue when lift animation is disabled. If the view observes a floor jump, the scene can reconstruct the intermediate landings along its direction and place an observed key change at the exchange. An excursion that reverses entirely between delivered observations cannot be reconstructed from endpoint observations alone.

In Chromium, a fixed-step replay queued the sequence 1 → 2 → 3 → 4 → 3 with two hoist trips around the handoff. Each hoist moved only at its acknowledged floor. The bridge circuit energized on the frame before either car moved away from the exchange. The active lever switched with the seated key, and the open-gate snapshot applied only on the final visit to level 3.

### Counterweight Hotel partner crossing poses

Parlando 0.5.0 exposes game actions and observations on its authenticated participant socket. The hotel uses this channel for changed crossing poses, sampled at no more than four regular updates per second and quantized to five centimeters and 0.05 radians. The server binds reports to the acting assignment and accepts them only with that guest’s bridge gate open and before their refuge arrival. The latest sample follows the standard reconnect snapshot. Horizontal coordinates use the common crossing frame so the rendered guest acquires the local deck elevation in either wing. A return to the cabin or confirmation of arrival clears the crossing pose.

The existing channel records accepted samples in session events and delivers them to the agent runner. The hotel porter skips a response to the human guest’s pose updates. A future transient participant transport could reduce event volume while preserving authenticated scoping. The controls and completion predicates remain independent of pose.

### Counterweight Hotel refuge entrance and interaction visibility

The furnished lodge door owns its own pickable leaf, glazing and brass push bar. Leaving these meshes out of the facade batch gives them one hover outline and one action. A guest within reach can click the door or use the matching dock button. Walking onto the rectangular mat submits the same arrival. This uses the porch's rotated coordinates and keeps an unacknowledged arrival eligible for retry. Only the server marks a guest safe and completes the booking when both have arrived.

Hover remains the usual way to inspect an object. Guests can explicitly enable outlines for available instruments and suitcases with Show interactables. The door at the refuge uses the same mechanism. The first guest can look back from a reserved place on the porch while their partner crosses.

Both roof sockets project onto the ravine-facing elevation, where the suspension rope runs over two exposed sheaves. The falls end at their respective car roofs. Running the sheaves from the same displayed lift height as the cars keeps the wheels, rope and animated brake stroke in sync. The glazed counter-car rides on the existing partner transform with its furnishings and actual suitcase load. The hotel-side desk window and the glazing give the bridge a sightline into both rooms.

### Solo porter crossing the refuge gallery

A solo guest needs to see the porter traverse the same gallery and porch that two human guests share. The browser and porter read the six centreline joints from the same route file. The porter samples each straight run at intervals of at most 0.65 metres and advances after its own accepted action, with 250 milliseconds between live samples. Each bend therefore has an explicit position and facing, and the final arrival uses the ordinary refuge action. Incoming human footsteps leave the porter’s pacing intact. A human pair incurs no porter timer. The fixed centreline makes solo travel predictable and ties both implementations to changes in lodge placement. The concurrent-footstep test exercises all joints with a second moving guest.

### Walking pace in the Counterweight Hotel

Manual and guided travel use a top speed of 3.5 metres per second. Manual input reaches that speed in about 0.22 seconds and stops on release, supporting short movements near instruments. Up to 100 milliseconds of frame time is integrated in collision steps of at most 20 milliseconds so ordinary frame-rate dips preserve walking pace and doorway clearance.

### Counterweight Hotel rescue galleries in the shared shaft frame

The two inboard cabin gates open onto separate timber galleries between the cars, each at its own dock height. East reaches the bridgehead directly. West crosses below East and climbs an exterior switchback, keeping the passage between the moving cars level. The bridgehead, span and lodge are all fixed in the East frame and booked at the East rescue floor. The span runs across the two towers' broadside, and the lookout on the porch aims between the bridgehead and the occupied cars. West's steeper booking climbs sixteen metres, so its guided traversal takes longer and covers visibly more ground.

Both browser navigation and porter samples use role-specific planar joints followed by the common span and porch. The browser derives the West ramp height from its horizontal run and the difference between dock elevations. Each receiver uses the sender's wing to recover deck height where the two approaches overlap in plan. A local camera stays in its own cabin's frame, applying the wing transform to send poses and recover crossing geometry. The vertical separation therefore needs no new network coordinate.

### Two occupied suites on one hotel cable

Each browser places both full cabin assemblies in the East-oriented hotel frame. The West frame is a half-turn at (5.12, 9.42) metres, which brings the two off-centre exchange sockets onto the same line at Floor 2. Changing participant seats changes which car owns the camera and controls. The opposite car has the same shell, service doors, glazed panels, furnishings, brake equipment and baggage rack as its occupied counterpart. Its wing finish, gate position and suitcase count follow that guest’s observation-visible state. The observation includes both wings’ fixed rack counts so the outside view can show the same suitcases as the rooms.

The existing server cable coordinate supplies both heights. A release advances the loaded system one landing under its brake and stop rules. The client interpolates one displacement for the pair, so the room-floor heights sum to sixteen metres throughout travel. The fixed architecture has separate service terraces, shaft gates and rails. A glazed baggage return and open rack frames expose the moving cars to the bridge. Private ledger contents stay in the guest’s own room.

### West hotel pavilion and exchange landings

The West service wing has a masonry envelope with large observation bays at each landing and a pitched roof over the residential side. Its open exchange portal and separate machinery roof preserve hatch, gallery and cable clearance. A fixed masonry landing spans the gap between the shaft fronts at each stop. Each slab carries a compass inlay centered below the two exchange hatches. Every inlay uses one print and the same orientation, with East pointing from the West shaft toward East.

The rescue approaches still use their booked heights and existing walking joints. Their steel dock plates close the gap from each cabin sill to the stone, and the timber decks start at the far edge of the stone. Shared landing dimensions keep these surfaces flush and leave the compass uncovered. The bearing course projects below the plates and stops outside the moving cabin floors.

### Counterweight Hotel bridge gate instruments

The West Room key socket supplies both gate releases after the exchange. Each gate has a circuit, gangway and seal readout in a two-sided instrument over the grille. Its electrical readout changes with the existing seated-key animation and the West contactor. Docking follows lift playback, while each gate takes its own latch and inspection spring reading from the authoritative observation. The observation includes the other gate's spring state so a wrong seal cannot look like an electrical fault or a simultaneous trip of both gates. Neither instrument reveals the correct seal. The controls retain the same rules, and the bridge inspection panel gives equivalent text readings.

### Counterweight Hotel evacuation and room names

East Room and West Room identify persistent guest spaces while lift and cable describe their machinery. A cold radiator carries the night desk’s avalanche bulletin in each room. The on-object paper and inspection view use the same document artwork. Broken steam pipes on the terrace and smoke and firewood at the patrol refuge connect the hotel’s loss of heat to the visible destination. These cues give the evacuation a reason without disclosing the gate solution. The print also opens through the ledger for keyboard players.

The gate readings occupy the lintel directly over each grille. A view from the brake showed the wall sconce obscuring a jamb-mounted prototype.

### Cross-posted rescue bridge inspection signs

Each guest has a single signed route record for the room across the shaft. The wall print, magnified print, accessible description and keyboard control identify the receiving room. The inspection sign also carries the suitcase manifest on supply routes, including the East dispensary pickup. The house bulletin has independent keyboard access. Removing the duplicate desk register and the purely decorative climbing exhibit leaves the wall signs as distinct landmarks.
 The workshop plate occupies a continuous opaque wall. A separate papered pier carries the route sign between two valley windows.

### Flush compass paving
The exchange landing is cut around a circular compass. The opaque disk and flagstone surround share perimeter vertices at the walking elevation. The slab supplies only its sides and underside. This removes the rotated quad’s black corners and competing top faces. Sizing the disk from the shorter landing dimension leaves a continuous stone margin at both thresholds.

### Joining the West pavilion to the hotel
The terrace facade previously stopped just beyond the door jamb, leaving open sky between the gated landings and the pavilion corner. A masonry return with a recessed desk window now closes that bay on every West floor. A fixed, five-storey connecting range joins the shaft houses beside the exchange landings. Its walls and floor slabs use the fixed wing and landing coordinates so the connection stays in place during lift travel. The guest routes still use the original bridge and terrace openings.

### Route-sign wall joinery
The old full-depth brass jambs overlapped the wallpaper with identical face depths. The replacement divides the wall into a central papered pier and two equally sized sash openings. Stiles and rails meet edge to edge around recessed glass. The header spans the whole assembly and the existing dado carries it below. Both room meshes and their signs share the pier center.

### Bridge doorway running clearance
The retractable steel plate now spans the actual 130 mm gap from the parquet edge to the landing on either car. Its endpoints use the physical floor geometry and wing transform, independent of the walking guide. The East stringcourse and West lintel below each stop finish 140 mm below the landing, clearing the 100 mm plate. Formerly their top surfaces coincided with the steel and the outer edge of the stone at upper stops.

### Render deployment for Counterweight Hotel
Build the locked server and web app against the monorepo SDKs, including the shared bridge route in the Rust stage. Restrict Docker context inputs to source and build manifests so the local dashboard login, database and experiment exports stay on the development machine. The single Starter instance uses a 1 GB disk, with an application-owned directory created after mount. The service drops privileges before starting Parlando. Render provisions a new catalogue independently of the development server.

### Free Render demo
The hotel Blueprint now selects Free compute and requests no disk. The container runs as parlando and writes SQLite into its writable home directory. Local state is ephemeral. Durable storage requires a separately configured store or a paid persistent disk.

### Hotel participant waiting room
The shared participant view labels the seats relative to the connected guest. The microphone status stays visible and a disclosure holds transcription diagnostics. The hotel groups the authoritative timer and recruitment guidance in one inset, ahead of seat status and secondary controls. The countdown uses timer semantics to avoid announcing each tick.

### Parlando logos and official screen palette (2026-11-20)

The canonical assets are `brand/parlando-mark.svg`, `parlando-wordmark.svg`, and `palette.css` in the same directory. The five-piece lowercase p refers to the tangram communication task of Clark and Wilkes-Gibbs. Use the wordmark in product headers and the standalone p in compact contexts such as favicons. Both use the same Futura Bold p outline, white interior seams, and a thin charcoal outline around the letter. The remaining letters are outlined paths so rendering needs no installed font. Preserve the wordmark’s spacing, x-height and proportions.

| Tile | Official sRGB | Interface role |
| --- | --- | --- |
| Stem, UdS blue | `#004875` | Primary action, link, selection |
| Left, forest green | `#33684B` | Active, success |
| Top, ochre | `#AD790C` | Draft and warning accents |
| Right, brick red | `#A04432` | Danger and error |
| Bottom, plum | `#71456E` | Testing and secondary emphasis |

The blue matches the solid pixels in the supplied UdS-Logo-Leitfarbe-blau-transparent.png (RGB 0, 72, 117). The published style guide gives #004877 and the supplied JPEG uses Adobe RGB. For screen assets use the PNG value. Neutral text, backgrounds, and lighter state fills support legibility. Status labels carry the meaning alongside color. Preserve study-specific colors where they encode game mechanics or experimental stimuli.

The repository-root `brand/` directory owns the originals so designers and contributors can find them directly. Run `make sync-brand` after editing the originals. It copies both logos and the palette to the Rust crate and Hugo static assets, and the standalone mark to the participant SDK. Local copies keep each subproject independently buildable.
The participant SDK bundles the same mark alongside the lobby module, so its platform label works in independently hosted game clients.

### Participant entry typography
On entry, the game name supplies the page heading. The admission button names the action. Hotel entry styles use the entry action group to scope the layout and give the consent text, microphone status and admission controls separate spacing. The shared shell continues to identify the institution, and configuration determines which form sections appear.

The hotel local build symlinks the compiled SDK with npm’s --no-install-links option. An installed packed copy could retain old markup after a same-version source rebuild. The Docker build installs into a fresh stage.

## 2026-11-20 — Prepare the coordinated 0.6.0 release

The replay renderer changes the public React and Rust game contracts. The release therefore uses
0.6.0 for the runtime and browser SDK. The upgrade guide covers the snapshot interface, state
deserialization, and the change from per-action snapshots to an initial state and accepted actions.

The examples use compatible package ranges at the coordinated version. Their local builds pass
the sibling SDK directly to npm with `--no-save`. A clean install verified that npm reused the
locked versions of other dependencies and left the registry lockfile intact. The hotel links
the sibling SDK for local rebuilds and installs a packed copy in the fresh Docker stage.
The Last Screening and Counterweight Hotel use the shared renderer directly. The hotel keeps
replay versions 6, 4, and 5 for state, actions, and rules in both build modes. Removing the
local-only feature preserves compatibility with sessions recorded by recent local hotel builds.

The first-party registry npm lockfiles still identify 0.5.0 until 0.6.0 is available on npm.
Once both packages are published, resolve those lockfiles through npm, build the registry
consumers, and run the release metadata audit with `--published --require-migration-guide`.

A temporary Last Screening build linked to published 0.5.0 created a session with one accepted
action. The 0.6.0 build opened a SQLite backup of that database and preserved counts, schema
version 16, integrity, foreign keys, the historical session detail, and its action state in the
corpus. That session returned the expected replay-unavailable response. A new session replayed
an action for both roles and exported a null action state.

Candidate validation passed the metadata audit, `make test`, and all five layers of
`make test-e2e`, including 13 Chromium lifecycle and 35 Prolific scenarios. The all-features
release build, Hugo build, package archives, and publication dry runs also passed. Clean
consumers compiled against the two locally packed artifacts, and the examples built with the
local SDK. Docker was unavailable because no local daemon was listening. The published build
paths and registry lockfile audit require the actual 0.6.0 artifacts on both registries.

## 2026-11-21 — Compare game execution through the existing runtime stress harness

A shared Rust fixture provides identical game rules and projections in the native
and Wasm runs. The opt-in Wasmtime adapter compiles a core module once and gives
each session a separate store. The test-only JSON ABI serializes each game method
independently, allowing the existing typed Game trait, storage, WebSockets, and
full-duplex audio path to run as usual. This tests the cost of the simplest
integration, including serialization and copying. A stateful single-call adapter
or a WIT component could have different costs. Only locally built trusted
fixtures belong here. Handling arbitrary game uploads would also require execution
budgets and a recoverable guest failure boundary.

Action latency measures from the client send to the matching role-A transition
during steady traffic. Polling while waiting for the next PCM frame preserves
audio pacing. The report includes missed matches to expose window censoring.
Live scheduling varies between runs even with identical inputs. Child CPU seconds
include server startup, compilation, and drain. Resource samples follow drain,
so RSS cannot establish a per-active-room allocation. The macOS sampler converts
Mach absolute ticks using the hardware timebase. The ps fallback reads cumulative
TIME instead of percent CPU. The pilot protocol and measurements are in
`notes/wasm-stress-pilot/`.

An initial human-human pair exposed a race during churn. The stress client
requested the new audio grant after the game WebSocket upgrade but before
receiving its active participant snapshot. During that window the room could
still be paused for reconnect, and the audio endpoint correctly returned HTTP
400. Churn now waits for the active game snapshot before requesting audio.
Both initial runs are retained in `notes/wasm-stress-pilot/initial-reconnect-diagnosis/`
and excluded from the paired measurements.

All six corrected 12-room runs passed. The paired change in p95 ranged from
−0.97 to +1.78 ms, and server CPU increased in each pair. Per-run measurements
and raw report links are in `notes/wasm-stress-pilot/results.md`.

## 2026-11-21 — Commit live turns before publishing their checkpoints

The room lock covers the stateful step, storage, and publication. The action task
owns that lock independently of the submitting socket, so cancellation cannot
interrupt installation after a durable write. The host keeps the last committed
checkpoint until storage confirms the accepted event batch and terminal result.
Reconnects take the room lock, and observations and affordances project from
the host's committed checkpoint.

A UUID on the accepted event identifies one prepared turn without a schema
change. SQLite checks the ID under its writer reservation, before checking the
lifecycle. The host retries the same event payloads and terminal value, including
after a commit failure that a subsequent transaction proves absent. If SQLite
cannot establish the outcome, the coordinator retains the room lock and checks
again. Action callers receive an error while confirmation is pending. The
periodic lifecycle sweep defers locked rooms so unrelated rooms can still
expire. Once storage proves rollback and the write attempts are exhausted, the
next submitted action still receives the prior checkpoint.

## 2026-11-21 — Exercise the production component host in the stress harness

The stress Wasm server registers the production component factory through the
same generic server startup as the typed native fixture. Both share mechanics
and deterministic workload agents. At the component boundary the agent
serializes its typed mark as JSON. The fixture computes both views in one step
and keeps its ordinary state inside the instance. A checkpoint every 32
accepted actions bounds reconstruction from the preceding checkpoint. Native
and component observations each include only the recipient's detailed history.
The matching headless test compares complete decision traces and completion.
The guest test build can trap after clearing its state to verify that a
replacement reconstructs the latest accepted frame. Benchmark builds omit
fault injection.

## 2026-11-21 — Keep mutable game state in a component and select durable frames in the host

The WIT world in rust-server/wit/game.wit defines init, restore and step. The
same session-specific instance advances once for an ordinary turn in either
runner. A returned frame carries both private views, neutral metadata and a
completion result. Queries during publication and reconnect read the selected
frame without a guest call. Choices, when exposed, belong in the role view.
Native games retain their existing typed trait and agent interfaces. Game::step
adds the distinction between an expected rules rejection and an execution fault
to the existing host paths, including historical reconstruction and headless
trajectory capture.

Deterministic components can omit checkpoints. Each accepted action adds a
constant-time immutable history entry to the candidate recovery recipe. A
matching selected frame reuses the warm guest. A different frame or trap
requires fresh linear memory and recovery from the most recent checkpoint or
the recorded initialization and subsequent accepted actions. The per-frame
cache identity is distinct even for two branches at the same depth and is
regenerated on deserialization. Longer periods between snapshots save copying
at the expense of memory for history and slower recovery. Nondeterministic
transitions require explicit complete snapshots. The host commits those
snapshots alongside the turn; a marked frame with missing data fails replay.
The public corpus projection excludes private recovery data.

The component package loader and CLI are opt-in. Each store has no host imports,
bounded memory, and per-call fuel. A SHA-256 check protects replay frames and
headless result reuse from a different component with the same game version.
The CLI loads one local game package per process and uses the existing live
database startup policy. Hot installation of multiple packages, process-crash
resumption of active rooms and a component-native training reward interface
would be separate changes.

The `make test-components` target builds both the Rust fixture and a minimal
guest written directly against the canonical ABI in WebAssembly text. The
second guest avoids a common binding generator between host and both test
guests. Its fixed ABI layout is specific to the versioned WIT fixture. The
Rust test guest can also return an invalid role view after modifying its state.
The host must discard that instance and recover the committed frame before
accepting another turn.

Verification included the shared native/guest headless decision trace, a
separately authored WAT component, two isolated sessions, recovery from a guest
trap, exhausted fuel, malformed output after mutation, and an uninstalled turn.
Live SQLite fault tests held an actual commit acknowledgement while cancelling
its submitter, and checked one durable record and one publication. Another
stored a non-reproducible transition and verified checkpoint replay, corruption
detection and corpus privacy. The component CLI was exercised with two live
WebSocket participants and a headless Python gRPC policy. These are correctness
checks; the earlier stateless benchmark numbers do not characterize this ABI.

## 2026-11-21 — Binary role observations on the component boundary

The stateful fixture profile attributed 4.11 µs per accepted headless action
to encoding the result and installing guest state. The entire hot component
call's bindings and payload transfer took about 0.61 µs. WIT v2 keeps the
one-step component interface and changes just the two role views to MessagePack
byte lists. Typed Rust game code can use the component SDK's named-field Serde
encoder. Other languages can encode the documented JSON subset directly.
Configuration, actions, completion, transition metadata, and opaque checkpoints
retain their existing representations.

The host decodes and validates both roles before installing a frame. It shares
each decoded role view as an immutable Arc<Value> with local Rust agents.
Explicitly typed component-agent callbacks need the new Rust observation type.
Browsers, gRPC agents and experiment records receive their usual values. Wire
decoding is bounded per view to constrain allocation on untrusted guest data.
Direct guest encoding assumes JSON-shaped Serde data. Types relying on JSON
coercions can materialize a serde_json::Value first, at the cost of allocating
that intermediate value. Compiled v1 guests require a matching v1 host for
historical replay and a rebuild for new v2 sessions.

The comparison uses the previous JSON component, the new component, and a native
fixture. Its in-process policies exercise both ignored and consumed observations.
The additional Hotel workload uses sampled role views and synthetic transitions.
Experiment sources, protocols, and measurements are under
`notes/wasm-codec-performance/`.

On the M5, the original one-worker workload improved from 110,422 to 165,129
actions/s (631,092 native). The matched hot interface subtotal fell from 5.86
to 3.63 µs. The four-way test kept execution and agent inputs fixed while
varying encoding and sharing independently. MessagePack and sharing both helped
the string-heavy fixture. On the nested Hotel snapshots, JSON plus sharing
reached 43,151 actions/s versus 39,993 for MessagePack plus sharing and 37,102
for JSON plus cloning. A single preferred codec for every view shape is still
an open question. New fixed-load live runs completed successfully, with higher
client p95 than the preceding same-seed JSON runs. The live comparison used
historical controls and does not establish causation.

## 2026-11-21 — Keep game execution typed through the local policy and learner

The v2 boundary eliminated JSON text for views but still allocated two decoded
JSON trees every accepted turn. Hotel and Last Screening also generated trees
for their native agents, which immediately deserialized them again. Component
WIT v3 represents config, actions, both role observations, completion and
metadata as MessagePack. The host validates each returned payload before
installing the frame and shares its original buffer with local agents. They can
decode into a game type or borrow selected strings and omit fields from a
policy-specific struct. An opaque checkpoint stays game-defined. The fixture
keeps its existing JSON checkpoint encoding. Replay pins the original component
digest; a v2 component needs its matching host.

Local rollout, training and trace capture retain typed observations and actions,
including native Rust structs or shared ComponentValue handles. A training
plan resolves its reward configuration before running sessions. The learner
registration supplies a decoder for pending batches resumed from JSON result
files, so observation deserialization is a requirement for learners and not for
evaluation-only games. Full traces can reference the learner input already
owned by a captured transition. Output serialization reconstructs the previous
trace and trajectory shape. WebSocket, gRPC and storage adapters may convert
data at their boundaries; the component recovery snapshot preserves the old
compact JSON-text action/config fields, including key order.

Both bundled games now project typed role-private observations for local
policies. The complete Hotel opening views match the v2 recorded fixture.
The cost of validation and policy-requested decoding remains part of the game
step or the subsequent callback. A policy that requests the whole view pays
for that decode. See `notes/wasm-component-v3/` for the headless measurements.

On one M5 worker the repeated original fixture rose from 177,252 to 210,062
Wasm actions/s (614,398 native). Reading the Hotel-shaped opening views rose
from 37,356 to 76,938 Wasm actions/s; the new typed native comparison reached
595,786 actions/s. Hotel includes the removal of the guest/native projection
trees in addition to the component ABI change. Both roles matched their v2
digests. Verification exercised the complete component suite, both native game
adapters, the cue-choice learner, the live WebSocket client and administrator
replay, and headless gRPC including result resume and digest rejection.

### Trust codec-produced values across the v3 component interface

Both endpoints use a codec that produces one JSON-compatible MessagePack value
per observation, action, configuration, completion, or metadata field. The host
adopts buffers received from that codec after checking their length (1 byte
through 8 MiB). It retains each role's immutable observation and decodes only
for a consumer. Session state, replay selection and opaque guest checkpoints
use the existing v3 rules. Game authors continue to use ordinary Serde types
with the component SDK. The public Game trait and the WIT function signatures
stay the same.

The SDK writer enforces the value-domain and resource rules as it encodes.
The separate raw-byte constructor still validates data with unknown
provenance. A third-party language binding is responsible for enforcing the
same rules before returning bytes. In particular, the host no longer promises
to detect a malformed, unused in-bounds observation when installing a frame.
The live event writer asks the game for metadata through an infallible optional
field. Unreadable metadata from a nonconforming encoder is logged and omitted
at that point. A headless rollout that ignores metadata never builds its JSON
representation. Persisted records and external protocols retain their existing
formats.

Measuring the encoder and receiver together was important. A first checked
encoder rebuilt map key sets and used the same expensive bookkeeping for
arrays and structs. The retained implementation writes typed MessagePack
directly and keeps separate state for arrays, named structs, and dynamic maps.
For small named structs a bitmap detects potentially repeated names and an
exact string comparison resolves collisions. This keeps custom serializers
subject to the same uniqueness rules as derived serializers.

In ten paired release trials, the median paired throughput gain was 24% for
the original ignore policy and 68% for policies reading the nested Hotel
fixture. The Hotel policy checksums matched the original implementation for
both seats in every trial. See `notes/trusted-codec/` for the release build
identifiers, exact workloads and raw samples. These figures describe local
agent rollouts; live web and remote-agent tests exercised correctness.

### Positional observation schema pilot

The local rollout pilot in `notes/declared-wire/` generated matching guest
writers and host readers from the stress and Hotel observation declarations.
It sends every struct slot in declaration order. An absent optional value
occupies a nil slot. Unit-only enums use numeric tags; an enum whose variants
can carry data uses an array containing the tag and all variant fields. Plain
strings remain ordinary MessagePack strings. All 514 named Hotel role views
matched their independently reconstructed compact equivalents. The study
keeps the production component contract intact. A deployed positional codec
would need an agreed schema/version and a schema-aware conversion back to the
existing named representation for recording and external clients.

### Discover each game's compact wire schema at load time

The v4 component exports describe(). The host calls it once on a disposable
instance before accepting sessions, validates its JSON declaration, and shares
the resulting schema with all sessions for that component. A separate instance
ensures that initialization cannot depend on side effects of discovery. Using a
declaration allows one host binary to load unrelated games, including non-Rust
guests, without linking their types or sending field names with every view.

Struct fields occupy fixed positions and enum variants have numeric tags.
Arbitrary strings and dynamic map keys keep their actual contents. The host
can borrow string data from an observation for a local policy. A ComponentValue
keeps its compact bytes and slot schema together, so serialization at a public
boundary reconstructs named fields and variants. Incoming named MessagePack
actions and configurations map directly into the declared representation.
Serde projections can skip unrequested fields. Reads bound structurally skipped
data and the aggregate text synthesized from declared names. Guests using the
provided codecs are trusted to encode the declared types.

A schema explains representations, not game semantics. Games still own state,
action acceptance, checkpoints, and both role views. Public and persistent
formats retain named values. Version 3 components require a version 3 host,
including for historical replay. Keep the matching artifacts when upgrading.
The tradeoff for independent game deployment is interpreting a loaded schema
while reading values on the host. The full measurements and reproduction
scripts are in notes/declared-generic/.

Twelve paired release trials per workload gave 160,665 Hotel actions/s for
the generic host, 135,369 for named v3, 205,197 for the game-specific positional
reader, and 629,177 native (medians). Both roles' full Hotel observations used
54,873 bytes across one session, down from 374,324. All 514 Hotel and 514 stress
views agreed. The simpler workloads lost 4–7% relative to named v3. This
benchmark uses the Hotel opening projections driven by stress-fixture actions.
Its throughput gap does not isolate the choice of Wasm execution engine.

### Decode declared Rust guest actions from their wire slots

The guest knows its Rust action type when it is compiled. A separate
`ComponentDecode` derive lets it opt in to constructing that type from the
fixed slots and numeric enum ordinals already defined by `ComponentWire`.
The guest does not need a cached runtime schema or a Serde visitor for the
declared portion of the action. The stress fixture uses `decode_declared`
at its step boundary, including the component compiled with trap variants.

The reader shares the MessagePack token, byte, node, and depth checks used by
schema decoding, and validates all slots before returning the constructed
value. Nested dynamic JSON uses the existing named reader on the same cursor.
String borrows come from the supplied action buffer. The separate derive
allows callers that rely on custom Serde behavior or runtime declarations to
keep using the schema adapter. It adds generated construction code to the
guest and requires nested declared types to support direct decoding. The
public names, wire bytes, and ABI version are independent of this choice.

## Resolved observation policies

The declared observation reader presents the public field names and enum
tagging through Serde. Hotel policies that visit an entire action catalogue
need its variant names and selected scalars from the surrounding view. Repeating
field and enum dispatch for each observation made these reads a measurable
part of the local rollout.

The component SDK resolves an explicit `Selection` with
`Projection::resolve`, using the guest's declaration for each seat. It sorts
record and tuple selections by their compact positions and prepares a table
from enum ordinals to names and payload lengths. Reads use the existing bounded
wire cursor and deliver borrowed strings and variant labels to the caller. A
read checks schema identity before walking the bytes and permits independently
owned but equal declarations. The normal host path shares the same schema
allocation, so it takes the identity fast path.

The caller sees values in wire order and chooses the projected inputs and
their keys. Options without a value emit no inputs. The API streams selected
values, so callers can accumulate into local state and publish only after
validation succeeds. The decoder still walks preceding values to find later
slots and checks the physical structure of unselected fields. Enum payloads
remain skippable when the caller selects only the variant label. The named
Serde interface continues to handle arbitrary deserialization and structured
payloads. List element boundaries, map selection, and enum payload projection
can be added if a policy needs them.

### Host several WASM games in one Parlando installation

The live component command binds one component factory, dashboard, and database
at startup. A long-running host should register immutable releases above the
existing game installations, preserve each game's database and dashboard, and
bind each experiment revision to both the mechanics and browser build. A game
admission gate can close independently of experiment activation and drain sessions
on their pinned releases. The additional catalog and release retention are the
cost of restarting and rolling back without losing provenance.

Use a versioned ZIP bundle and a scoped REST API with independent upload and
activation permissions. Install provider credentials once at the host, with
per-game grants enforced by host adapters. Serve game JavaScript on participant
origins separated from the dashboard. The first deployment model assumes reviewed
releases. Supporting unreviewed publishers requires stronger process isolation
and validation of the existing trusted component codec boundary. The design and
verification cases are in [multi-game-host-design.md](multi-game-host-design.md).

### Verify the final component rollout path against the previous v4 build

For the stress fixture, history stores StressRole values which serialize to
the original role strings. Native apply_action retains the immutable caller
contract. The stateful guest validates before writing and retains the evicted
history and previous scalar values until both role views and any checkpoint
have encoded. A recoverable projection failure rolls back without cloning
every history entry. The immutable native comparison also benefits from
copying typed history rather than allocating per-entry strings.

Use the resolved policy reader for stable local scalar selections and the
derived guest reader when the action type is known at compile time. Neither
choice alters the v4 ABI or the named protocol for remote agents. The local
reader test keeps the same policy outputs and visits all observations for both
seats. On 16 paired release rounds, Hotel progressed from 155,807 to 236,808
actions/s and the updated native fixture reached 844,548. Median paired
speedup was 1.55× against the prior v4 implementation and the paired native
lead was 3.52×. This is traffic from Hotel opening views with stress mechanics.
Measured with the older host, the fixture's history change improved throughput
by 19%, with a further 7% after enabling the direct action decoder. The
resolved reader added 23% on the completed guest. These are separate paired
comparisons. See notes/final-rollout/ for protocol, raw data and stage probes.

### Demonstrate the v4 component contract with Last Token in C

Use a standalone game under `experiments/c-last-token` to exercise the
language boundary of the existing component host. Generate the canonical
ABI from the checkout's WIT with `wit-bindgen c`. Compile with Zig's C driver
and use its libc for ownership-compatible buffers. The packaged guest has
zero host imports. The registry Rust release 0.6.0 lacks component hosting,
so build the host from this checkout. The React app consumes the registry
client 0.6.0.

Keep the guest codec limited to the game's bounded compact MessagePack
schema. The schema and encoder evolve together, so the host probe checks
named payloads and rejection codes. With at most 15 accepted moves,
deterministic replay is sufficient for recovery. Larger games may need
checkpoints to bound re-execution. The release benchmark uses the actual
component factory from the checkout and measures load, fresh session
initialization, and current-state turns. It leaves transport, persistence,
and named observation decoding outside the timed sections.

### Resolve shared provider credentials through explicit grants

Use operator-owned provider profiles for the authenticated destination and
secret reference, with per-game grants and usage limits. The current adapter
sends the bearer key to the configured URL, so experiment endpoint overrides
cannot govern a shared credential. Resolve the key on new connections to
support rotation without redeployment. Upstream costs and compromise affect
every game using the same key.

Keep the bundle tool artifact-based. Game toolchains produce a WASM component
and static assets. One packer and host-compatible validator serve manual and
automated deployment. This keeps project build scripts off the host and lets
CI isolate upload credentials from build steps. See the
[multi-game host design](multi-game-host-design.md).

### Test import-free Python mechanics against the existing component host

Port Last Token to Python to distinguish a WASI compatibility issue from
resource limits. The port uses generated WIT bindings, the same compact
MessagePack schema as the C version, and deterministic replay. It reuses
the participant bundle and the same host probe. Its manifest uses a
separate game ID to keep its component provenance distinct.

With componentize-py 0.25.1 and `--stub-wasi`, the component validates and
exports the expected WIT with zero imports. The stock host fails when it
tries to instantiate core instance 17. Componentize-py 0.17.2, 0.19.3,
and 0.24.0 also hit that limit. The 0.17.2 test uses the older generated
binding module for `Err`. The 0.25.1 artifact contains 38 core module
instantiations.

A temporary host copy under `/private/tmp` raises the Wasmtime store
instance cap from 16 to 64. The Python component passes the host probe
with that single adjustment, including both winners, action rejection,
session isolation, and recovery by replay. The host in the repository
retains its current limits and empty linker. If Parlando adopts this build
path, consider a higher instance allowance for reviewed components after
measuring the effect of concurrent Python sessions. That decision is
independent of granting WASI capabilities.

On Apple M5 with Wasmtime 36.0.23, both guests used the same temporary
release host. The probe takes 21 factory load measurements and reports
medians of 21 batch means for session and turn measurements after one
warmup batch. Each batch contains 1,000 12-move games. Timings cover the
in-process host only.

| Measure | C median (min–max) | Python median (min–max) |
| --- | ---: | ---: |
| Component bytes | 7,388 | 18,456,593 |
| Factory load, ms | 2.982 (2.701–4.042) | 4,338.250 (4,161.408–4,849.305) |
| Session creation and initialization, µs | 6.935 (6.736–8.659) | 671.105 (630.338–832.098) |
| Current-state accepted turn, µs | 0.376 (0.369–0.381) | 9.727 (9.262–11.320) |

Build with `componentize-py==0.25.1` on PATH and
`experiments/c-last-token/python/build.sh`. The existing probe takes an
optional component path after `check` or `bench`. Pass
`experiments/c-last-token/build/last-token-python.wasm` to the stock probe
to reproduce the load failure or to a probe linked to the temporary
64-instance host to reproduce the successful checks and timings. The
Python manifest uses the existing Last Token client bundle.

The temporary host also passed a live desktop and phone-width Chromium
smoke test. Two participants paired, saw matching pile updates and
role-appropriate turn controls, and received the same winner.

## 2026-11-21: Separate editable component projects from deployable packages

Context: Game authors need source-level validation and a portable artifact whose identity and paths
do not depend on their local Cargo or npm installations.

Decision: The project manifest resolves source and output paths relative to itself. Builds compile a
Rust cdylib with the import-free WebAssembly target and use Cargo's artifact records to locate it,
then wrap its embedded WIT interface with wasm-tools. The build validates it with the same factory
used by both runners. It stages the component, manifest and compiled browser assets under
`dist/parlando`. The staged manifest carries only the public game metadata and relative artifact
paths. A build lock serializes replacements and a failed subprocess leaves the preceding package
available. JSON command output has a single envelope on stdout and child diagnostics go to stderr.

Decision: Scenario tests submit named JSON values through the real component runtime. Cases may
request a fresh engine session plus restoration of persisted state before a move. The starter uses
deterministic replay from the recorded seed and accepted actions. An uninterrupted control session
receives accepted actions only. Comparing role views, completion and transition metadata with that
control exposes hidden side effects of rejection and discrepancies after recovery. A native test
independently exercises the same game trait.

Tradeoffs: Web compilation requires the Rust WebAssembly target and wasm-tools. Browser builds
install npm dependencies and use the lockfile once generated. The Rust scaffold honors an explicit
`--sdk-path`. Otherwise it copies the necessary SDK manifests, sources and WIT from the SDK bundled
beside the CLI or its source checkout, so the generated project can move without machine-specific
paths. Without an available source, the generated registry dependency requires a published SDK
release. The local live server continues to
use the administrative dashboard to configure and open experiments.

## 2026-11-21 — Export typed guest mechanics and deploy immutable packages

A Rust component implements a small, stateful game trait. The SDK owns the WIT
exports, schema description, typed input decoding, and frame construction.
Role views, completion and metadata can borrow session data while the SDK encodes
them. Checkpoints are optional and opaque. Stateful games retain deterministic
replay and need neither a Clone implementation nor a general-purpose state codec.
The guest and server each package their own WIT definition, with a workspace
test for byte equality. Ordinary rejections leave the guest reusable. Failure
to encode a frame after state mutation traps so the host discards that instance;
a fixture exercises recovery to the last committed action.

The local server and hosted deployment use the same component loader. The
deployment host validates packages, includes participant files in the revision
digest, and pins each experiment to one revision. Selection and intake have a
separate operator credential. Browser JavaScript is trusted by the operator
selecting a revision and runs on the installation's web origin. Distinct asset
bundles are not separate browser security origins. Deploy independent trust
domains on separate origins. One instance owns the durable volume, including
package revisions, selections, and per-experiment databases. Intake is closed
on startup so recovery does not admit participants without operator action.

Store upload credentials per origin and game in Keychain or the desktop Secret
Service. CI-injected credentials must name their receiving origin. Uploads use
the same noninteractive command that developers use, while provider credentials
are confined to infrastructure provisioning. Use a manual artifact build for
CLI bundles and host images, keeping artifact generation separate from package
publication. Source deployment uses the provider's volume for SQLite and
package files. Live cloud provisioning and registry publication require the
operator's account and are separate from local conformance and integration tests.

### Measure concurrent Python Last Token sessions

Use the same temporary 64-instance host from the Python compatibility
test. The harness loads one shared factory and holds 16, 64, 128, or 256
session stores and their states in one process. It initializes them
serially, then four workers advance disjoint sets of sessions over 12
synchronized rounds. Each round submits one move per game. Every session
must report player B as the winner at the end. The macOS stress probe
samples process resident memory and physical footprint after initialization
and during play. It checks a 3.25 GiB footprint budget every 16 session
initializations and caps the test at 256 sessions.

The host ran on a 32 GiB Apple M5. Both Python runs completed all sessions.
Ranges below span the two runs. Each baseline was collected after loading
one factory and running the correctness checks.

| Open games | Python midpoint footprint, MiB | Python turns/s | C midpoint footprint, MiB |
| ---: | ---: | ---: | ---: |
| 0 | 166–169 | — | 5.1 |
| 16 | 351–353 | 16,910–115,701 | 6.2 |
| 64 | 904–907 | 24,951–140,759 | 9.5 |
| 128 | 1,670–1,686 | 22,626–136,742 | 14.0 |
| 256 | 3,196–3,222 | 34,483–94,463 | 22.9 |

Python added approximately 11.8–11.9 MiB per session above the factory
baseline at 256 sessions. The slowest round of 256 turns took 9.47–28.95 ms.
Other workloads ran on the same machine, and scheduler contention can
affect those timings. The experiment includes engine work and synchronization.
It excludes networking, SQLite, agents, and sustained churn, so the maximum
tested tier is a lower bound for this particular component and harness.

The separate instance cap limits the number of core Wasm instances a single
store may allocate. Raising it to 64 permits the tested Python component
while preserving the import boundary. A deployment's game admission budget
should account for observed per-game memory and leave headroom for the rest
of the server, independent of the instance cap.

Reproduce on macOS with the Last Token probe linked to the temporary
64-instance host. Run it with `stress experiments/c-last-token/build/last-token-python.wasm`
and pass `experiments/c-last-token/build/last-token.wasm` for the C control.
The probe implementation is in `experiments/c-last-token/probe/src/stress.rs`.

## Parlando Game SDK naming and derivation

Expose the Rust guest library as parlando-game-sdk. GameValue describes a type
in the public game interface and derives its schema and encoder together. Emit
the direct decoder with field-specific trait bounds, so input types are checked
by the Game trait and an output containing borrowed slices can be written without
providing a decoder. Preserve the v4 WIT contract and existing encoded values.

### Add Python authoring to the component SDK and CLI

Raise the core instance cap from 16 to 64 per Wasmtime store. The earlier
Python probe established that the CPython component exceeds 16 and runs at
64 with the existing memory and fuel budgets. Keep the empty linker and
compile Python with `componentize-py --stub-wasi`. This change expands the
number of Wasm modules one session may instantiate. Session admission and
memory budgeting remain separate concerns.

The Python SDK specializes the existing v4 game world using six Python
payload annotations. One compiled type description drives schema emission
and the bounded MessagePack codec. Guest state lives in a session object
owned by each component instance. A rule rejection returns the stable
application reason before state mutation. Encoding or projection failure
after a successful turn traps, which gives the host its normal recovery
path. Generated WIT types are imported by the guest adapter and native
unit tests can import the same game module. A game with external package
dependencies must load its dependencies at build time and remain within
the capabilities provided by the component.

The CLI dispatches build and native test commands through the manifest's
language-specific authoring inputs. Both languages use the same loader,
scenario runner and package output. The portable CLI archive carries
matching SDK sources and the scaffold vendors the Python package. Beacon
Relay uses the same participant view and YAML scenario in both languages.
Its target derives from the session seed and it recovers through accepted
action replay.

The packaged CLI was tested from a moved Python scaffold. Verification
covers SDK wire and lifecycle tests, native Beacon tests, the shared
scenario against the exact packaged Wasm, zero Wasm imports, and a
two-browser live game through the packaged CLI. The Rust component
conformance suite provides regression coverage for the shared host.

## Use Wasm as the server game contract

The live server and headless runner previously supported a native Rust implementation as well as a Wasm implementation behind game and factory traits. Both now hold a compiled LoadedGame, open an isolated GameSession per play-through and consume GameFrame observations and completion. The Rust Game authoring trait belongs to the guest SDK. Native Rust policies can still register in a thin host or a headless runner.

The guest owns mutable state. A frame records the identity needed to recover the committed position after a trap or failed write, using a checkpoint when available and deterministic replay otherwise. Successful writes need no second guest invocation. Choices a game needs to disclose travel in its role views. The store and participant protocols retain their existing formats. Pre-Wasm recordings need their original executable and data store for replay; an unrelated module cannot reconstruct their private initial state. Bundled game launchers use separate component databases.

The live installation adds the SHA-256 of the loaded component to the existing
JSON build manifest. It accepts an object or null and rejects a supplied digest
that differs from the executable bytes. Standalone, embedded and deployed
launchers publish that provenance through the installation manifest. The live
test suite embeds a prebuilt WebAssembly fixture to exercise production frame
projection, failed writes, checkpoint replay and agent startup during Rust test
runs.

### Headless agents and Cue Choice

The headless runner accepts a compiled component alongside deployment metadata
and stores the digest calculated from the executable in the run manifest. A
worker shares that compiled component across plans and uses one isolated game
session per plan. Resumed runs validate the saved component digest before
reusing finalized results or training batches. Outputs created by an earlier
native binary lack that digest and require a fresh run directory.

Agent callbacks and learner trajectory payloads use the host's concrete
ComponentValue. The game can include a legal-action catalogue in a role view.
Cue Choice does this for the learner and signals the dealer phase in the dealer
view. The Python policy still applies the legal set at selection time while
retaining the same model prompt. The v1 artifact and gRPC fields for an
additional action catalogue remain optional and new host decisions leave them
empty. Rewards can inspect role views, transition metadata and completion
through GameFrame. Private state stays in the guest.

The headless lifecycle tests embed a stripped build of the stress fixture's
headless example. Ordinary host tests therefore exercise a real guest without
invoking the Rust Wasm toolchain at test time. Rebuild the fixture when changing
the game interface or test mechanics using the commands documented beside the
fixture loader.

### Use the same game SDK import name in Python and Rust

Expose `parlando_game_sdk` as the Python module so examples use the same
game SDK name in both languages. The Python distribution, source directory,
and CLI vendor directory are named `parlando-python-sdk` to identify the
Python package alongside the Rust crate. The scaffold, archive, and wheel
all supply the new module name.

### Package the four authored Rust games as components

Space Game, The Great Tree, Counterweight Hotel, and The Last Screening now
build Wasm from the same Rust mechanics their rule tests exercise. The guest
owns mutable state and uses the recorded seed. The host recovers through action
replay. The HTTP protocol and WIT need no game-specific extensions. Space Game
and Screening carry legal controls in each role projection. Hotel already
carried them in its view. Great Tree continues to validate controls when acted
upon. Screening's compact role enum retains the flattened public JSON fields
used by its browser and authored agent.

The optional native launchers register each game's existing agent factories
with the shared host. They load the exact same component as the generic CLI.
The hosted agents retain their typed policy logic and convert values at the host
callback boundary. Great Tree's headless launcher uses the same pattern.

Each default local launcher writes to `.local/component.sqlite`. Included
container recipes use a separate component SQLite path. Historical installations
keep their native records and stores. Opening those records through the new
guest is outside this transition because the old state snapshot format has no
component replay provenance.

Native rule suites and four component scenarios verified role views, seeded
starts, accepted and rejected actions, and recovery through the guest ABI.
Clean npm builds verified the four packages against the matching published
0.6.0 client lockfiles.

### Publish all reusable registry packages from Make

Use one serial publication sequence for the derive crate, Rust Game SDK,
server, browser client, Python Game SDK, and Python agent SDK. Keep the
server and browser client in lockstep. SDKs retain their independent
versions, so dependency crates already available in the Cargo index can
be reused and Twine skips existing PyPI distribution files. Named Make
targets support recovery after a partial release.

The dry-run target builds and checks each Python wheel and source
distribution without invoking upload. Cargo dry runs use the registry
package when its workspace dependencies are indexed. For a new dependency
chain, the dry run lists the dependent crate contents and the maintainer
runs its full verification after its dependency becomes available.

The three optional authored configuration fields are represented as optional in
the compact schema. The guest assigns the previous default when no value is
supplied. Declaring an ordinary required field with only a Serde default was
insufficient because the host validates named configuration before invoking the
guest. All four packages passed fresh-database startup and HTTP health checks.
The Space Game Linux package built as a static x86-64 executable and passed its
manifest checksum verification. The Docker daemon was unavailable for image
builds.

### Version all registry packages together

All six Parlando packages carry the same release version. Align the SDK
manifests with the existing server and client version, 0.6.0, as a repository
metadata change. The next release increments the shared version and
publishes all six packages even when some package source has not changed.
Use exact Rust SDK dependency versions to prevent Cargo from pairing a
server or SDK with a different release. The CLI derives its scaffold SDK
dependency from its own version.

Make checks the package versions, Rust SDK pins and npm lockfile before
uploading. Standard publication attempts each package at the shared version.
Use individual targets to recover a partial release.

### Extend the deployed-experiment host

The checkout already has a long-running host with game-scoped REST uploads,
content-addressed packages, immutable experiment selections, per-experiment
SQLite databases and explicit intake. Preserve those contracts. A per-game
dashboard can index the current selections and reuse applicable experiment
views without consolidating databases or adding a second release lifecycle.
Put the dashboard on an operator origin. Participant origins may be shared
within a trust domain, with distinct origins for independent publishers.

The hosted selection path rejects inline keys. Use a host-owned Speechmatics
binding through the existing transcription-provider seam and check the same
binding at activation and stream admission. Keep the existing package format
and CLI, including the Python build path. Revisit binary upload transport only
if package size or handoff warrants it. The revised
[multi-game host design](multi-game-host-design.md) records remaining work and
verification cases.

### Scope credential sharing across integrations

Use one operator-managed registry for outbound credential references, typed
service profiles and game or experiment grants. Apply it to Speechmatics,
ElevenLabs, Prolific and remote-agent transport. Check allowed operations and
upstream resources as well as endpoint identity. The same Prolific token can
back distinct study grants, and a remote-agent transport key is independent
of the model keys held by that agent. Keep deployment, admin and participant
authentication in their existing scopes. This extends the earlier
Speechmatics-only integration plan. See the
[multi-game host design](multi-game-host-design.md).

### Keep remote agents game-scoped

Remote agents are defined against a game's action and observation contract.
Register each agent endpoint and its transport credential under one game ID.
Experiments of that game may select compatible registrations. The host may
resolve references through the same secret backend used for shared provider
credentials. Remote-agent registrations and transport credentials are outside
the cross-game provider grant model described in the preceding decision.

### Scope researcher access at the host

Use one named dashboard login for a single operator. With multiple
experimenters, authorize an individual identity against host-owned game and
experiment grants and record the actor for lifecycle and data operations.
Prefer existing institutional identity for login. An experiment assignment
does not imply export permission. Start with operator-managed grants and
reuse the game-scoped deployment tokens for machine uploads.

### Let an experimenter deploy and update an assigned experiment

Authorize deployment against the experiment as well as the package's game ID.
Keep a stable experiment identity and participant entry URL. Record immutable
deployments under that identity and switch only the named experiment's
selection. Reuse existing selection database isolation for deployment data.
The first cutover closes intake and drains live sessions. It retains the
history and checks the expected selection before switching. Experiment-scoped
machine credentials can deploy within the same limits. Host operators retain
namespace, access and provider authority. This supersedes the proposal to
reserve release selection for operators and require a new public experiment
ID for each update.

### Start with lab-wide experimenter access

For a group whose members can share study data and operational authority,
start with a lab-wide experimenter role and an infrastructure operator role.
Keep individual identities for audit and revocation through existing sign-in
when available. Finer grants follow project access boundaries. Self-service
deployment can use a fresh immutable run ID under the same game. Introduce
in-place deployment switching when a participant link must survive an
update.

### Use individual passwords with host-wide experimenter access

The lab wants separate named password accounts for attribution and revocation,
with the same access to every game and experiment. Store accounts at the host
and reuse the local authenticator's Argon2id password hashing and session
mechanisms where applicable. Replace the earlier human assignment-grant
proposal with host-wide role checks. Keep a separate operator role for account
and infrastructure management. Provider grants and scoped machine deployment
tokens retain their own authorization boundaries.

### Implement named hosted accounts and a shared lab dashboard

The host can expose a browser dashboard on a dedicated hostname while
preserving the participant and bearer-token API routes. Cookie sessions are
host-only and authenticated mutations require the configured Origin and a
CSRF token. The separate hostname is required because uploaded participant
HTML and JavaScript are controlled by game authors. Use the infrastructure
bearer token to provision the first operator account, then let named
operators create, reset, and disable accounts through the dashboard.

Store users, revocable sessions and audit events in a separate host
dashboard SQLite database. Passwords use Argon2id with bounded verification
work. Sessions hold only token digests at rest and expire after 12 hours.
Reset and disable revoke sessions transactionally. Durable audit attempts
precede side effects and outcomes are written after the result. Run data
and immutable selections keep their existing storage and lifecycle.
Experimenters share authority across all games and runs. Browser deployment
uses the existing immutable package envelope and creates a fresh run when
switching builds. No experiment databases need conversion.

### Make the hosted manual CLI-first

The 0.7.0 experimenter manual assumes the installed CLI and uses Beacon
Relay throughout. Order the public workflow around a local pilot, the
portable WASM package, shared-host deployment, then data collection and
export. Keep operator bootstrap separate from the experimenter's upload
and intake steps. The shared host uses immutable run IDs and named accounts.
Describe provider-backed flows under the single-game host where their
credential editor exists.

Use the Hugo manual as the canonical task guide. Keep current technical
references and institutional privacy materials in docs. Remove superseded
single-game deployment tutorials, historical migrations, duplicate manual
chapters, and their obsolete screenshots. The docs assume the 0.7.0
release. Smoke tests exercise the checkout CLI with local SDK sources
because that release is awaiting publication.

### Keep the game creation skill technical

Use the installed CLI and the project scaffold as the working examples.
Default new mechanics to Rust, honor an explicit Python request, and keep an
existing project's language unless asked to port it. The supplied specification
owns game design. The skill covers the Wasm contract, role-safe browser views,
testing, and packaging. Agent and deployment instructions live in conditional
references. The local skill is a symlink to the repository version, so one
edit updates both entry points.

### Keep Cranelift and fuel for agent experiments

Full agent–agent play through the production experiment runner on Apple M5
covered both Hotel routes and both Screening seat assignments with bundled
agents. Compare component backends only after checking full role inputs,
decisions and outcomes for identical seeds. Wasmtime 36.0.23 Winch loaded the
components faster and changed the seeded starting observations in both games.
Pulley preserved the traces and delivered 13–21% of Cranelift throughput.

Keep the per-call fuel budget for deterministic interruption and replay.
Cranelift with an epoch watchdog reached a 1.5–10.1% higher paired median and
Cranelift with no execution budget reached a 1.1–14.6% higher paired median
across the four workload and concurrency combinations. Each combination had
paired rounds on either side of baseline. The epoch option trades a fixed
instruction budget for a timing-dependent deadline. Revisit on the target
deployment after profiling workloads that spend more time in guest execution.
The benchmark scripts, full methodology and samples are in
`notes/wasm-engine-evaluation/`.

## Make Wasmtime fuel accounting configurable for agent–agent workloads

Realistic agent–agent workload measurements above justify an opt-in unmetered mode for trusted components. Keep the 50-million-unit per-call budget by default. Select instrumentation when compiling the component because raising the fuel allowance still incurs accounting overhead. Use one mode throughout an experiment, including training, validation and replay within the run. Record a non-default mode in the experiment fingerprint while preserving the existing fingerprint for default fuel. This keeps existing outputs resumable and prevents mixing modes in one result directory.

Live servers select the policy at launch. Memory and instance limits are independent of fuel. With fuel disabled a non-returning guest can occupy its worker indefinitely. No wall-clock interruption replaces the deterministic budget in this mode.

## 2026-11-22 — Select a single participant communication channel per experiment

The participant shell previously rendered its text composer even when voice transport was enabled. A browser could also submit typed dialogue to the game socket in a speech condition. The experiment now pins one communication mode, typed (the default) or speech. Speech transport, recognition, and synthesis settings remain revisioned when dormant. The effective mode controls validation, provider initialization, audio-plan issuance, public participant UI, and admission of human-authored messages. The origin remains on the research record. Agents keep their existing speaker-and-text observation for typed messages and final transcripts.

The React package owns one communication widget, its styles, and its actions. The default shell mounts it for games that use standard placement. With communicationPlacement="game", the renderer mounts it inside its layout. React context supplies the same session capabilities in either position and keeps presentation and transport out of the game observation schema. The replay uses the experiment revision that produced the session and shows recorded messages without live controls. Stable CSS hooks and theme properties allow game styling. Existing built participant assets must be rebuilt against the matching client before deployment with this server.

### Workspace data conversion

Schema 17 replaces voice.enabled in the stored experiment configuration and every configuration revision. A true flag maps to speech and a false flag maps to typed. The previous schema allowed typed input in either condition. This correspondence identifies the intended transport setting, not the origin of every historical utterance. Recorded events retain their original content and attribution.

Before conversion, the source stores were checked for open database, WAL, and SHM handles. Each database was backed up alongside its source and converted transactionally. The backups are:

- `games/last-screening/.local/screening.sqlite.schema16-before-communication-20261122T065523Z.backup` (2 experiments, 3 revisions, 0 sessions).
- `games/counterweight-hotel/.local/hotel.sqlite.schema16-before-communication-20261122T065523Z.backup` (3 experiments, 6 revisions, 46 sessions, 4,506 events).
- `games/great-tree/server/parlando-great-tree.sqlite.schema16-before-communication-20261122T065523Z.backup` (4 experiments, 18 revisions, 50 sessions, 1,640 events).

The source, backup, and converted databases passed SQLite integrity and foreign-key checks. Each converted configuration matched the original with only the communication mapping applied. Each experiment matched its current revision, and every session retained a valid revision. Per-table row counts were preserved apart from the new schema-history record. The older schema-10 Space Game database and disposable smoke-test and screenshot databases were outside this conversion.

Shared hosts store immutable selection files separately and derive each database filename from the serialized selection hash. No shared-host selection store was found in this workspace. An operator upgrading a populated shared host must convert its saved selection and database together while the host is stopped and preserve the database under the updated selection hash; changing just the selection would select an empty store.

## 2026-11-22 — Exercise typed games in the runtime stress harness

The stress workload previously exercised the speech channel for both seat pairings. The communication condition belongs to an experiment, so each harness invocation now creates its own typed or speech experiment. Speech remains the default for continuity with existing capacity runs. The four-condition Make target runs separate experiments in succession. Its reports keep provider costs and message counts attributable to the relevant mode.

Typed runs use the production participant and game endpoints, including the normal agent callback when one seat is an agent. In addition to observing delivery on the game sockets, the runner checks the durable conversation and terminal outcome of the fixture games. Synthetic recognition and synthesis processes run only for speech. Browser playthroughs remain a separate check on the real widget and participant interface; the load clients exercise the same server using sockets. A passing short matrix establishes correctness at its stated concurrency and duration.

The prerelease gate runs the four-condition stress matrix and repeats the typed conversation at the package boundary. Both the installed Rust and Python Beacon Relay starters are deployed as explicitly typed experiments. Two independent browser contexts exchange text in both directions and finish each game. The package test catches failures in UI composition or npm packaging that a socket client cannot observe, while the stress runs check concurrent delivery, agent responses, and storage.

## 2026-11-22 — Confirm project creation in the terminal

The CLI renders the successful scaffold result as a short confirmation with the project and manifest paths. Formatting at the CLI boundary keeps both paths tied to the scaffold result, including custom destinations, and preserves the existing structured envelope for callers that pass `--json`.

## 2026-11-22 — Unpublished starter dependencies

The 0.7.0 CLI can be used locally before the matching browser client reaches npm.
The CLI looks beside its executable for `share/parlando/<CLI version>/`. When that
directory contains the matching npm tarball, scaffolding copies it to the
project's `vendor/` and uses a relative `file:` dependency from `client/package.json`.
This keeps the scaffold movable and lets npm record the tarball integrity in its
normal lockfile. A CLI without the tarball generates the exact release version
as a registry dependency. Python setup instructions use the SDK version
independently of how npm obtains the client.

Explicit guest SDK paths continue to take priority. The versioned asset directory
also accepts the matching Rust and Python guest SDK sources. Existing portable
CLI archives supply SDKs under `share/`, so that location remains available
before the source checkout fallback. Local prerelease bundles need to be restaged
if their source artifacts change, even when the prerelease version stays the same.
The 0.7.0 bundle was installed at
`/Users/koller/.cargo/bin/share/parlando/0.7.0`. Its npm tarball SHA-256 is
`2f8339c4a8ed8149f52eac83ac77e22bbbac37f476c4e36c9ed899661947cb64`.

## 2026-11-22 — Separate server startup, deployment and intake in the CLI

The local tutorial used to start a game-specific server from a package manifest. Local pilots now use the same deployment host as shared installations. The CLI exposes that host through `serve --store`. Deploying records an immutable revision, selecting pins it to an experiment, and changing intake admits participants. This adds setup steps to a first pilot and makes it possible to inspect and retry each operation independently. A custom Rust application can still embed the game runtime.

Each CLI operation supplies its terminal summary alongside its structured result. The terminal can report domain-specific outcomes such as native tests skipped, scenario counts, deployment revision and participant URL. The global `--json` flag selects the result envelope for automation and subprocess diagnostics stay on stderr. Scripts that parsed the previous pretty-printed default must request JSON and read its `result` field.

## 2026-11-22 — Deploy the built game as a single ZIP

One upload must carry the exact component, game metadata and browser assets selected at build time. The CLI creates `dist/<game-id>-<game-version>.zip` using the manifest ID and semantic version and prints its absolute path after the build diagnostics. The same file is the input to package validation, the deployment CLI and the browser file picker.

The dashboard sends the ZIP bytes to the host with the existing session and CSRF credentials. The host reads the manifest to determine the game in the installation-wide dashboard. On a game-specific route the server checks the ID against the route and its credential scope. Package revisions still identify the member paths and bytes, so archive compression does not change a pin. The service unpacks accepted contents into its existing immutable revision store. The builder stages the ZIP under the output directory and replaces the same game/version atomically after validation. Other versions keep their filenames. Project-based deployment resolves the exact current ID and version. A previously built directory must be rebuilt as a ZIP for a new upload. Existing server-side package revisions remain usable.

CLI success messages describe the state reached by the command. The build confirmation states that the ZIP is ready for deployment and leaves the path on its own final line.

## 2026-11-22 — Default host store for local pilots

The host CLI uses its working directory as the default store. The tutorial explicitly creates a sibling host directory and starts the service there in a subshell, keeping the interactive shell in the game project for deployment. Starting the CLI in a different directory therefore selects a different store. Startup diagnostics display the resolved path. Operators can pin an absolute location with `--store` independently of the service working directory.

A host can start without any game or operator credential. In this mode the existing selection loader restores participant routes, while the absence of an expected bearer digest or a dashboard session keeps the management endpoints closed. Configured CLI upload grants and dashboard bootstrap still require their normal credentials. Credential configuration takes effect at startup.

On a default loopback listener the CLI enables the administrator dashboard on localhost and prints its URL after restoring experiments. The participant origin uses the numeric loopback address. For an empty account store, the local terminal receives a per-process setup link whose fragment authorizes only the first operator creation. The dashboard clears the fragment and uses ordinary password login thereafter. Explicitly configured external hosts retain their operator-token bootstrap. Binding first preserves the assigned port in generated URLs.

## 2026-11-22 — Preserve dashboard setup on reload and separate local logins

The first-operator link is scrubbed from the address bar immediately. The setup form keeps its credential in the administrator origin’s tab session storage until setup finishes, is cancelled, or the server rejects the credential. This lets the user reload the form. Browsers that block session storage can still complete setup on the original page.

Browser cookies are shared across ports on the same hostname. Dashboards on nonstandard ports now use a stable cookie name derived from the canonical administrator origin for login, lookup, and logout. Existing sessions on those ports require one new sign-in. The standard HTTP and HTTPS ports keep their existing cookie names.

## 2026-11-22 — One administrator URL for local hosting

The default loopback host prints its plain `/admin` URL on every start. The page checks the account store when a visitor has no session and offers first-operator creation only while the store is empty. This supersedes the terminal setup link and tab storage described above. First-use authorization now trusts a local client reaching the configured administrator origin. A second local user could claim an empty store first. Explicitly configured hosts retain their operator-token bootstrap. The database still arbitrates concurrent first-account requests.

## 2026-11-22 — Reuse the Parlando administrator identity

The local dashboard serves the existing Parlando wordmark, mark and palette from the administrator origin. Its account creation and sign-in pages follow the existing administrator page. Browser labels and instructions call the privileged account an administrator. The stored role and API value remain `operator`, so existing privileges and account records retain their meaning.

## 2026-11-22 — Organize the administrator site around games

The administrator landing page lists games once per game ID. Opening a game takes the user to its existing scoped URL, where its revisions and experiments can be managed together. User administration lives in its own view. Audit history loads on demand there. This keeps account and audit queries off the Games page and preserves links to individual game pages.

## 2026-11-22 — Display the manifest name in game administration

The project and ZIP already declare a public `name` and an immutable game `id`. The dashboard reads the name from installed package manifests and uses it for the game list, game heading and browser tab. A smaller ID distinguishes games with the same name. Upload confirmations use the name in that upload. Experiment pins, URLs, grants and archive filenames continue to use IDs or digests.

A game may be renamed across revisions, including builds with the same semantic version. The catalogue takes its name from the distinct revision with the latest revision-directory modification time, using the greatest digest to break a tie. An identical re-upload reuses its directory. This avoids a separate mutable naming record and makes already-installed games readable on upgrade. Copying stores without preserving directory timestamps can alter which label wins when names differ.

## 2026-11-22 — Create an experiment first and configure it on its own page

Game pages collect only the new experiment ID and its package revision in a modal. A successful creation opens the same administrator URL as Manage in the experiment list, `/experiments/<id>/admin`. Choosing a revision from its row preselects that revision. The separate participant URL continues to resolve under `/e/<id>/`.

The creation POST reserves a fresh ID, records defaults and leaves intake inactive. Required game-specific values can be supplied on the experiment page. The CLI's selection PUT continues to support an explicit initial configuration and idempotent retries. The initial selection hash still identifies the same SQLite database. Later settings saves use its existing configuration revision records, with an expected revision and inactive-state checks. Participant sessions remain associated with their originating runtime revision.

## 2026-11-22 — Explain experiment IDs before submission

Hosted experiment IDs form single path segments in participant and administrator URLs and in the host store. The creation dialog describes the existing identifier rules and suggests a valid spelling for spaces. The user retains control of the ID that will be shared and persisted.

## 2026-11-22 — One creation button on the game page

The Experiments section opens the creation dialog. Its package revision picker handles all builds, so the Revisions table only lists versions and digests.

## 2026-11-22 — Preserve the case of hosted experiment IDs

Hosted experiment IDs accept uppercase ASCII letters within the existing 64-character format. The host preserves the submitted spelling in URLs, selection filenames, and SQLite paths. It matches route IDs exactly. Game package IDs, grants, and host aliases retain their lowercase format.

Creating or pinning a case-only variant returns a conflict before building a runtime or writing files. This keeps the same store valid on case-sensitive and case-insensitive filesystems and leaves exact-spelling CLI retries idempotent. Startup also rejects case-only duplicates in stored selections, which can otherwise be introduced by editing or merging stores outside Parlando.

## 2026-11-22 — One current build and one installation per hosted game

The administrator dashboard opens the game’s existing multi-experiment installation. The installation reads and writes `games/<id>/game.sqlite` across deployments. Newly deployed games start inactive. A persistent activation marker records participant availability separately from experiment intake, and an idle replacement preserves the marker. Each participant URL uses `/e/<game-id>~<experiment-id>/` so games can reuse experiment IDs while ZIPs built against the existing JavaScript client still recognize a single experiment path segment. The administrator origin mounts the dashboard at `/<game-id>`.

The host prepares and validates a candidate before switching `packages/<id>/current` by atomic file rename. The pointer names one complete build directory. Once the directory update is durable, the host removes previous build directories. Startup prunes candidates left by interruption and requires offline conversion of stores containing legacy experiment pins or versioned package directories. The database retains recorded study metadata and results as the build changes.

Requests hold a per-game read lock through app dispatch. Replacement and availability changes hold the write lock while checking intake, sessions and connections and publishing the result. Upload publication also serializes competing uploads, including the first upload for a game, to avoid duplicate runtimes. The upload semaphore bounds archive memory while this work proceeds. Deactivation waits for intake to close and connections to drain. The administrator can continue to review settings and historical data while the game is inactive.

### Dashboard ownership and the local Beacon installation

The shared server overview links each game ID to its existing experiment administration application on the admin origin. Authentication and CSRF come from the host session. The participant origin serves package code. A one-use replay grant carrying the game, experiment and session lets existing ZIP clients read a replay for up to 60 seconds after issuance. Opening intake checks the current package configuration, while historical experiments remain available for inspection.

Converted the stopped pre-release store at `/private/tmp/parlando-test/beacon/exec` under its exclusive host lock. The byte-verified backup is `/private/tmp/parlando-test/beacon/exec-backup-20261122-203325`. The sole selection referenced the sole Beacon build, with SHA-256 revision `723f3284bdca93f5fef539ebc280825254d5a29f9315e85511ca74031861ce4f`. The selection’s configuration matched the saved experiment. Copied that build byte-for-byte and took a transactional SQLite snapshot into the per-game database. Set the game active to preserve its former participant URL availability under the new URL scheme. Its one experiment, `beacon-test`, remains inactive.

Verified database integrity and foreign keys, schema version 17, exact experiment database dump equality and every table count before switching directories. The database preserves the experiment, its configuration revision and the game settings row, with zero sessions. The unchanged account database contains one account, one login session and six audit events. The old structures exist only in the backup.

The Games overview shows the existing boolean availability as a text label with the same 7 px green or ochre marker as an active or inactive experiment. The decorative CSS marker needs no separate accessible label.

The planned sessions field explains the reporting target in the experiment configuration form. Its help uses a paired-game example to distinguish sessions from participant places and describes the verified Prolific fallback and continued intake after the target is reached.

## Beacon Relay participant styling

Both starter languages copy the same client stylesheet. The starter owns the visual theme and uses the participant runtime’s lobby, countdown, and seat classes so game authors can restyle them without replacing the waiting-room logic. Changes to these classes should be checked against the screenshot in the getting-started guide. The image was captured from an isolated two-browser pilot before the second seat joined.

## Reusing experiments after a game update

A game update retains the game’s database and experiment IDs. Dashboard readiness, editing, and intake use the installed game’s configuration validation across semantic versions. A compatible configuration can launch at its existing revision. An inactive experiment with incompatible game settings can be repaired through the ordinary revision-checked save. The experiment’s creation version remains provenance, while each new session records the installed game version. Reproducibility across updates depends on retaining the earlier game package.

## Participant recruitment in Beacon and other browser games

The host attaches the receiving seat’s admission source to waiting, active, and paused participant snapshots. A synthetic Prolific admission is a local preview. The JavaScript client exposes the source through GameSnapshot.recruitmentSource. Replay has no live participant and uses null. Terminal outcome text describes the game event, while the optional recipient-specific server handoff supplies the provider instructions. This keeps a direct participant’s exit message accurate on reload and after intake settings change.

## Missing upload tokens in the CLI

Credential lookups distinguish a missing game token from a credential-store failure. For a missing token, deploy prints the login command for the resolved server URL and the package’s game ID, plus the dashboard upload option. Other store failures retain their diagnostic context so login is suggested only when it addresses the failure.

## Browser-authorized CLI deployment

CLI sign-in uses a five-minute device pairing with the existing dashboard account. The browser requires a signed-in session, CSRF protection, and the code printed in the terminal. The CLI exchanges a separate device secret once and stores the resulting credential under the canonical server URL in the OS credential store. The account database holds only digests, bounds outstanding requests, and revokes CLI credentials and approved pending requests when a password changes or an account is disabled. Uploads use the same account-level authority as dashboard uploads and record the same audit identity. A host needs the separate administrator origin for this flow. Bootstrap operator access remains a separate server-management capability.

### Default CLI deployment host

Keep servers.yaml as a flat alias-to-URL mapping. Login, deployment and availability commands resolve the alias `default` when the target flag is omitted. That entry may select a named alias so a URL only needs to be maintained once. Existing URL-valued entries, including one named `default`, keep their meaning. Explicit URLs bypass configuration. A missing default produces an error rather than choosing a server implicitly.

CLI host aliases live in ~/.parlando/servers.yaml so the manual, help and lookup all point to one predictable location. Existing users of the former XDG location can move their servers.yaml file there.

CLI login links carry the short-lived approval code in the URL fragment. This identifies the requesting process across browser sign-in without typing a code or opening a local callback port. Approval requires an explicit authenticated click. Only the CLI holds the separate secret needed to retrieve the credential. Links from other people can identify their CLI, hence the prompt to approve only a login the user initiated.

The CLI approval page uses a compact card with a plain statement of the permission granted and a single explicit approval action. The header identifies the signed-in account.

## Installing WASM agents on the account-based deployment host

Game and agent authors share the Rust SDK, but deploy their packages independently. The host accepts an agent ZIP through the same authenticated upload endpoint as a game ZIP and records its policy revision against the installed game build. Replacing a policy requires the runtime to be idle. Persisting the registration lets the host reconstruct its agent factories after restart, while attaching it to the game build avoids silently applying a policy to a changed game schema.

Experiment operators choose Human vs agent and a factory in the game dashboard, save the configuration revision, and start intake there. The package rehearsal exercises this flow through the installed CLI and Chromium after testing the policy under both headless metering modes.

## Independent agent projects and game dependency changes

A participant in a shared task can start from the organizer's game ZIP. An agent project records the game ID plus its dependency locator and exact version in `agent.yaml`. The same entry holds the required SHA-256 of the archive bytes. Keeping the locator and digest together lets a single file replacement update the release pin. Local ZIPs are verified on use. URL releases are cached by digest for subsequent offline builds. All agent project builds resolve a game ZIP. Game authors can build a new local ZIP and select it with `parlando agent set-game`; a release server is optional. This adds an explicit packaging step during joint development and ensures participants and authors both discover the interface from the packaged WASM.

The resolver returns one loaded executable and its metadata. Binding generation uses the component's validated schema for actions, both role views and completion. Headless runs from the agent manifest compile the policy against that schema and give the same loaded game to the runner. This preserves the distinction between a schema-compatible change and a behavior change. The runtime keeps its existing package compatibility rules and checks the exported policy against the game.

The scaffold currently emits Rust and keeps the policy separate from its generated module. Local and downloaded ZIPs use the same package loader regardless of the game's implementation language. The optional bindings path belongs to the agent manifest so other layouts and projects that maintain their own wire types can use the same dependency workflow. A new agent language needs its own policy scaffolding and codec generator. A matching game-language SDK or the game's source tree is unnecessary when consuming a release ZIP.

Agent authoring commands share the `parlando agent` namespace. Creation uses `agent new` alongside `agent set-game`. The game scaffold keeps `parlando new NAME`, including when the game ID is `agent`.

Project build, check, and test use an explicit manifest when supplied, otherwise searching the working directory for game.yaml followed by agent.yaml. The order preserves existing game commands for games with a colocated policy, while standalone policies need no manifest flag. Agent creation uses the project ID as its destination when --path is omitted, matching game creation.

The hosting dashboard lists agent package IDs, names, versions, and revisions from the same ordered pins used to construct each hosted game runtime. Keeping this metadata with the runtime makes installs and game revision changes visible together and avoids rescanning the package store for each dashboard request. The browser keeps expansion state within the signed-in view and uses the successful upload receipt to reveal the specific agent revision.

A fresh agent scaffold uses the packaged action and seat-A observation declarations to select the tick-on-request policy when it can construct the single unsigned ticks field and read the boolean your_turn field. This works for renamed Beacon starters and across game languages. Other games retain the editable generic policy. The display name defaults to title-cased words from the factory ID and can be overridden with --name. Scaffolding writes the same name to the manifest and exported definition so the existing package identity check covers both dashboard surfaces.

The deploy command accepts the package ZIP as its positional argument. The existing --package option and no-argument game release lookup remain supported. Clap rejects combining the two explicit forms before the CLI accesses host credentials.

The hosting Games view uses a shared package column for human-readable names and stable IDs, and a release column for versions and revisions. A dedicated leading disclosure expands agent rows. Narrow screens reflow the same rows and actions into two lines. Upload and refresh live in the page toolbar, with the upload form opening above the list.

The hosting list groups games by a divider before each game and a branch beside its agents. Parent and child rows share a white surface so a persistent fill never competes with the temporary upload highlight.

Agent rows show identity and release only. Presence under a game already conveys installation. On phones these fields share one row, while the parent disclosure aligns with the game title independently of wrapped metadata.

The hosting dashboard renders one table body per game so the same game and agent rows form a card on phones and remain column-aligned on desktop. Phone cards put update and activation under Manage to leave room for release metadata at narrow widths. Upload, refresh, and game navigation remain directly accessible.

## 2026-11-23 — Railway GitHub deployments for the shared host

Use Railway's branch autodeploy with the root railway.json and existing shared-host image. This keeps GitHub pushes responsible for server updates and leaves game and WASM policy releases with Parlando's authenticated upload API. One replica owns the SQLite store. On Railway the entrypoint requires the volume at /data and dashboard configuration before serving, since a healthy empty or upload-disabled host would give operators a misleading deployment signal. Volume handoff interrupts service during redeployment. GitHub installation access, domains, DNS, volume attachment, and secrets are environment-specific. Retire the Render blueprints and provider URL fallback. Existing Render resources and research records require a separate operator cutover.

## First administrator setup on a shared host

Local and Railway dashboards use the same first-visitor claim on the admin origin. The atomic first-account insert closes setup across simultaneous requests and restarts. The admin must claim a fresh volume immediately. CLI pairing withholds its sign-in URL until an account exists. Account sign-in authorizes game uploads and activation, and account administration requires an admin browser session. This replaces the infrastructure bearer secret and uses the existing account schema.

## Railway shared-origin hosting for trusted games

Default the container dashboard to its resolved public URL. Railway supplies the public domain at runtime. Route the shared origin using the existing management and experiment paths and continue checking the configured authority, browser origin, session and CSRF values. Uploaded game clients share the browser privileges of signed-in users on that origin. The operator accepts this trust model. An explicitly configured separate admin hostname retains origin isolation for other deployments.

## First-party game and agent package builds

Build each game ZIP first. Source its browser SDK from this checkout so the host and participant runtime agree before a coordinated release. The browser projects use relative npm dependencies and build the SDK during their own prebuild. Docker stages preserve the same relative layout. The game launcher pins each local agent manifest to the archive reported by the CLI before building its independent ZIP. A single agent may live beside its game. Great Tree has separate agent directories so either agent can be built or installed on its own. Pins identify archive bytes, so a rebuild with different assets or toolchains requires refreshing the pin.

Space Game's movement agent requests host wakeups during live play and follows the runner's decisions during headless play. Root Bot shares its behavior between its component and its logged native implementation. Great Tree's OpenAI agent uses the native host's network and secret facilities. Cue Choice's Python learner uses the remote service interface, while its dealer can run in the same training coordinator from a ZIP or a native factory. Neither the learner nor the OpenAI backend can perform external I/O inside an import-free component. Last Token's Python build keeps its component inside the project directory so the same package path passes project validation and builds the ZIP.

The packaged checks cover seven game archives and six agent archives. Cue Choice exercised the packaged dealer with a random Python learner through checkpoint creation and held-out validation. Hotel and Screening self-play reached game completion. Space Game's two movement agents exchanged accepted moves and chat until the evaluation's message limit. They do not operate the station equipment needed for a mission win. Great Tree also uses the participant SDK's interaction flag to disable gate input during replay and reconnection.

## Build and deploy entry points for example games

Each example builds locally and deploys to a separately managed Parlando host. The build records the relative ZIP paths and digests reported or computed for that invocation in dist/deploy-packages.json. This lets deployment use the exact release and agent selection even when dist contains older builds. Deploy verifies the full list before the first upload and passes the game to the authenticated CLI before any agent. Each upload is a separate server operation. If a later upload fails, the earlier receipts still apply and the operator can retry the deployment. Activation and experiment intake remain separate operator actions. Native headless experiment binaries continue to serve their experiment-specific roles.

A deployment rehearsal built all seven example projects and used their deploy scripts against one disposable, authenticated localhost server. The hosting dashboard reported seven installed games and six agents. Separate preflight checks covered stale ZIP exclusion, default-host selection, and failure before uploading the game if a subsequent agent archive was missing or had changed.

## Hosting dashboard game controls

The Games list displays host availability and offers package updates directly at all screen sizes. Activation controls participant routing and has no runtime memory benefit. Operators can change it with the CLI. A newly installed game therefore needs CLI activation before its first pilot.

## Participant entry and microphone meter defaults

Beacon Relay’s theme provided the landing card but left voice preparation to the client. The only default meter rules were mounted inside the active communication widget, so preflight showed a label with a zero-height track. The shared meter now supplies its own track and fill, and the startup shell provides the voice and consent layout. Both use a CSS cascade layer so existing game themes can override the defaults. Game packages bundle the client at build time, so existing deployments require a rebuilt client bundle. The speech browser scenario checks the layout and a painted, nonzero reading before and after entering matchmaking. The waiting room previously omitted the meter. Both speech screens render it even when readings are unavailable, using the existing inactive zero level.

## Serve voice worklets as same-origin modules

Beacon Relay’s deployed Vite bundle inlined the small capture worklet as a data URL. Parlando’s participant Content Security Policy allows same-origin scripts and rejects that module when the voice transport starts. The microphone permission check and local level probe can both succeed before this failure. The shared audio sink marks both worklet URLs with Vite’s no-inline asset query. This keeps their URLs relative to the game bundle at any experiment mount path and prevents a change in either processor’s size from affecting whether it can load. Each module now needs its own asset request. Existing deployed games pick up the fix when their browser assets are rebuilt and deployed.

## Preserve hosted agents across game deployments (2026-11-23)

A fresh build previously started with an empty agent registry. Stage the current registrations alongside the new game and validate them before publishing the build pointer. This keeps game and agent versions independent, preserves exact agent revisions across restarts, and lets a failed compatibility check leave the previous build available. Registrations already lost under the former behavior can be restored by uploading the selected agent archives again.

## Detect participant SDK mismatches before packaging

The served Beacon 0.1.3 bundle matched the local ZIP and decoded voice.enabled, while the running experiment returned communication.mode=speech. Both seats could connect without a browser ever opening its audio transport. A transcription-gated session correctly waits for the human's provider readiness and the software seat's start callback. A game version bump does not select the browser SDK; registry and candidate tarballs can even have the same npm version. The build therefore exercises the installed official SDK's public configuration decoder after npm restores the lockfile. Custom clients and prebuilt distributions still need an artifact-level pilot.

The shared waiting room exposes pending audio readiness and lets a reloaded tab prepare its own microphone without registering another seat. The browser regression holds the audio ticket until both connections and a live local meter are observable, reloads the human tab, then permits audio and requires a successful first game action. The external transcription service is represented by its local ready event. The same test accepts an unpacked Beacon ZIP so package selection can be tested independently of the SDK workspace.

The participant-facing Beacon policy is beacon.tick_on_request. Its initial greeting covers speech and typed play. The release rehearsal deploys that generated policy and checks that B holds its turn until a finalized “Tick.” arrives over the transcription path. A separate headless test fixture sends commands to exercise the generated agent. The speech test uses a simulated provider and real browser audio transport, so it checks transcript delivery and agent response independently of acoustic recognition accuracy.

## Recruitment-specific participant copy

The waiting room uses the admitted seat’s recruitment_source exclusively. Falling back to experiment configuration when the source is missing could instruct a direct participant to return a submission. Terminal codes and links remain driven by the per-seat server handoff. The packaged-game regression checks direct exit copy both on leave and after reload; the older Beacon 0.1.3 bundle fails on its unconditional return instruction.

Beacon's request policy matches “charge the beacon” as a complete finalized utterance or typed message, ignoring case and terminal sentence punctuation. This avoids depending on the single syllable “tick,” reported as “take” in live recognition, while keeping one request per pulse and the existing factory ID. Recognition accuracy still depends on the deployed recognizer.

## Dashboard package drops

The Games dashboard presents a standalone drop target above the table. Dropping a ZIP starts deployment immediately. Clicking the target still opens the picker and review dialog. Both paths share validation and submission. Both new packages and revisions use the manifest-directed upload endpoint, so the games table needs no per-row Update action.

The shared speech widget no longer renders the remoteAudio receipt flag as “heard” or “silent.” This flag records transport activity independently of the microphone level or recognition of a command. The browser relay test observes incoming binary audio frames at both seats directly; the public audio status remains available to callers.

The dashboard agent release cell spans the Release and Status columns because activation status belongs to the game. This gives the upload highlight a painted cell across the entire row while retaining the two-item mobile layout.

The New experiment dialog uses the dashboard’s existing stacked field layout. The ID label names the input explicitly, and its help and validation message sit directly below it. Empty validation feedback takes no layout space.

## Deployed games and experiment intake

A successful deployment mounts the game and its experiment routes. Experiment lifecycle alone controls new participant admission. The host no longer reads or writes per-game active marker files. Old markers have no effect. This removes the second availability decision from the API, CLI, and games list. Existing session and package replacement guards still protect live work. No database schema changes are needed.


## 2026-11-23 — Consent templates and study information sites

The conductor dashboard already has an editable consent-template selector, and both the
browser and server enforce required consent before waiting-room admission. Adapt the two
pages from Wuhn/informed-consent-form into two required plain-text templates in that
selector. Preserve the section structure and declarations; substitute explicit placeholders
for upstream institution-specific laws, anonymity assertions, risk claims, and withdrawal
promises. Retain Apache-2.0 licensing and source blob identifiers under rust-server/third-party.
This reuses the existing exact-presentation consent records without a parallel form endpoint
or database representation. Long consent bodies preserve paragraph breaks in the client.

Each stored experiment exposes separate participants, data-protection, and conductors
information URLs. Three embedded static HTML documents deliberately contain empty titles
and bodies. The dispatcher checks the stored study and serves these documents before runtime
construction, so creation/cloning immediately provisions the URLs even when the study is
inactive or cannot start. Catalogue and creation responses supply deployment-aware URLs;
the dashboard links to them. No physical per-study file copies or database schema changes
are needed for empty documents. Future per-study content storage/editing remains a separate
decision; these public static sites must never become implicit access to admin data.

Tradeoff: the upstream pages are editable consent sections shown together, rather than a
Flask-style Next-page flow. Existing templates remain available; new templates are explicit
opt-ins, not silently assigned to existing studies. Blank sites are available for existing
stored studies as well. Public documentation describes this contract and the lack of a content
editor. The graft CLI and graph directory were absent, so context came from direct source
inspection and no graph refresh was possible.

Validation: all 138 browser-client tests and the TypeScript build passed. Dashboard module
and API checks passed, as did targeted Rust formatting checks. The multi-experiment routing
regression verifies all three empty HTML sites before activation, HEAD responses, rejected
POSTs, and unknown study/audience paths. It and all 11 admission regressions passed after
rerunning outside the sandbox, which blocks local WebSocket listener creation. Rust emitted
existing unused-import warnings in the test modules.


## 2026-11-23 — Empty experiment imprint

Add an imprint document to the existing static study-site registry. Creation and cloning
therefore expose its URL automatically, and the dashboard labels the link “Imprint”.
The embedded HTML title and body remain empty until content is supplied. Reusing the
registry also makes the page available to existing studies without database changes.
The tradeoff remains that content is embedded rather than editable per experiment;
future content ownership and editing will need a separate implementation. Extend the
existing routing regression to check imprint availability before activation and the
same GET/HEAD/POST behavior as the other information pages.

## 2026-11-23 — Beacon local browser SDK alignment

The locally installed server expects communication.mode, but the generated Beacon client
resolved the registry's 0.6.0 SDK, whose decoder lacks that contract. Build and pack this
checkout's js-client, retain its tarball in beacon/client/vendor, and pin Beacon's browser
dependency and lockfile to that local artifact. A tarball gives npm ci reproducible bytes
and avoids linked-package React resolution issues. This is a local candidate alignment,
not a publication or release version change. Regenerate the tarball explicitly when the
local browser SDK changes; the installed CLI's compatibility check remains enabled.


## 2026-11-23 — Privacy document navigation and downloads

Move the static study-page actions from the experiment header into a Study documents
section under Privacy. Give Data documentation and each static page an independent
accessible tab, retaining the existing generated report and its Markdown/JSON downloads.
Static pages remain empty; each has Open page and Download HTML actions. Refresh actions
when the selected experiment changes to avoid links to the previous study.

Serve HTML attachments through an authenticated installation-level admin route that
reuses the static document registry and verifies study existence. A public cross-origin
link with a download attribute is insufficient when the admin and participant origins
differ. The endpoint returns the same bytes as the public page with a study-specific
filename. No runtime activation, schema change, or content editor is needed.

Validation: dashboard syntax and Rust routing tests passed. Each of the four downloads
requires authentication, carries an HTML attachment filename, and matches the public
page bytes. A DOM smoke check passed for click/arrow-key tab selection, focus handling,
hosted admin prefixes, changed experiment links, and clearing links on deselection.


## Privacy document tab order

Following the clarified request, order the study document tabs Data documentation,
Participants, Study conductors, Data protection managers, then Imprint.
Data documentation is initially selected; panel order and keyboard navigation follow
the same sequence. This is a presentation change with no storage or API changes.


## 2026-11-23 — Country-specific study imprints

Context: Public annotation studies need an editable operator notice under participant
access and consent. Previously the per-study imprint route served an empty HTML file.
Only the shared institution field exists; no structured responsible-person details
were available to reuse.

Decision: Store `direct.imprint` (country, explicit public identity fields, authoritative
plain text) in the existing revisioned JSON configuration. Seed the operator from
shared institution settings. Provide DE, US, FR, AT, ES, IE, IT, NL and a clearly labeled
EU baseline. Keep the responsible contact distinct from the legal representative.
Generate on country selection and update untouched copy when contact inputs change;
confirm before replacing edited prose. Never infer public contact details from accounts.
Render only saved imprint text, HTML-escaped, on public GET/HEAD and authenticated HTML
downloads, without runtime activation. No database schema change or conversion is needed.

Tradeoffs: Plain text is intentionally editable and auditable, without HTML execution.
Templates contain conditional placeholders and are not compliance certification; operators
must review before saving because saving publishes the exact text, including placeholders.
Institution changes do not silently rewrite published notices. The existing information
page routes remain the public navigation contract; game clients control their own links.

Sources: EU Directive 2000/31/EC Article 5; German DDG §5;
https://entreprendre.service-public.gouv.fr/vosdroits/F37351 ;
https://www.usp.gv.at/themen/brancheninformationen/information-und-kommunikation/offenlegungspflicht-gemaess-para-25-mediengesetz.html ;
https://www.boe.es/buscar/act.php?id=BOE-A-2002-13758#a10 ;
https://cro.ie/Registration/Company/Incidental-Obligations/Letterheads/ ;
https://www.lab-to.camcom.it/media/events/attachment/Slide_Garelli.pdf ;
https://business.gov.nl/regulations/rules-business-correspondence/ .
Follow-up: Activity changes (selling, editorial publishing, regulated services) require
review beyond these annotation-study templates. Graft is not installed and no graft/
index exists in this checkout; targeted source inspection was used instead.

Validation: All five dashboard JavaScript test files passed, including the new imprint
editor behavior tests. Both Rust imprint unit tests passed. The compiled-game hosting
integration test passed, covering saved public text before activation, study isolation,
HTML download parity, and HEAD responses. It required an unsandboxed rerun because the
sandbox denied its local socket. Changed Rust files pass rustfmt; the repository-wide
format check also reported unrelated existing differences in app.rs, lib.rs, and
bin/parlando/project.rs, which were left untouched. Existing unused-import warnings remain.

## 2026-11-23 — Visible imprint regeneration outcomes

The regeneration button could silently write identical text (contact edits already
refresh untouched templates), or rely on a native browser confirmation for custom
copy. Replace native confirmation with an inline Replace/Cancel prompt and report
regenerated, already-current, missing-country, and cancelled outcomes in an accessible
status region. Stage country changes until replacement is approved, preserving edited
text and its original country on cancellation. No persistence or schema change.
The embedded dashboard assets require a rebuilt/restarted server to serve this change.
Validation: dashboard JavaScript tests passed, including direct button clicks, missing
country, no-change feedback, explicit replacement, and preservation on cancellation.
Graft remains unavailable; inspected the known imprint module directly.

## 2026-11-23 — Inline imprint document and automatic country templates

The Privacy → Imprint panel contained only instructional text even though saved copy
was already served publicly. Render the exact plain-text imprint as an escaped document
in that panel. Bind initial load, country generation, contact edits, and manual prose
edits to the preview; clear it on study switches and configuration-load failure.
Label unsaved copy as a draft, keeping the public route and download revision-backed.

At the user's request, remove the regeneration button, status controls, and all
replacement/cancellation prompts. Country selection directly generates its template
and the help text explains replacement. Contact changes still preserve customized
prose. This supersedes the previous regeneration-button decision; no storage changes.

Validation: all dashboard JavaScript test files passed. Five focused imprint tests
cover generation, prefilling, automatic updates, custom-copy behavior, preview updates,
escaping, saved/draft distinction, and removal of the regeneration controls. Graft
is still unavailable; used targeted reads of the existing implementation.

## 2026-11-24 — Country and legal references in generated imprints

Generated notice text now records the selected template label and jurisdiction-specific
legal references. These are part of the authoritative editable text, so the Privacy
preview, saved public page, and download carry them consistently. References are
qualified by applicability rather than asserting that every annotation study falls
within every cited law. The US entry states that there is no general federal imprint
requirement; it does not mislabel email or marketplace laws as annotation-site duties.
Existing saved notices are not rewritten. No schema changes.

Verified references against official legislation/government or chamber guidance:
- Germany: https://www.gesetze-im-internet.de/ddg/__5.html and MStV §18(1).
- France: https://entreprendre.service-public.gouv.fr/vosdroits/F37351 and LCEN.
- Austria: https://www.ris.bka.gv.at/eli/bgbl/i/2001/152/P5/NOR40025801 and the previously recorded §25 MedienG guidance.
- Spain: https://www.boe.es/buscar/act.php?id=BOE-A-2002-13758#a10 .
- Ireland: https://cea.gov.ie/en-ie/Publications/Case-Studies/Annual-Report-2025-Case-Studies and https://www.irishstatutebook.ie/eli/2003/si/68 .
- Italy: previously recorded Article 2250 chamber guidance; Article 7 D.Lgs.70/2003 at https://www.mn.camcom.gov.it/files/CommercioEstero/ecommerce-Imodulo21maggio2021.pdf .
- Netherlands: https://ondernemersplein.overheid.nl/wetten-en-regels/regels-voor-bedrijfscorrespondentie/ (BW 3:15d).
- EU: Article 5 Directive 2000/31/EC, subject to national implementation.

Validation: focused imprint tests and all dashboard JavaScript test files passed.
Graft remains unavailable; used targeted inspection of the known module.

## 2026-11-24 — Non-media references and linked imprint sources

Removed MStV §18(1) and MedienG §25 references at the user's request, along with the
Austrian media-owner prompt. Retained France's LCEN general online-provider disclosures:
these extend beyond editorial publishing. Each remaining jurisdictional reference has
official legislation or government/chamber guidance URLs in the generated text. The US
entry has no invented statutory source for a general federal imprint requirement.
Italy uses official chamber guidance because the attempted Normattiva article URLs
returned error pages. Source URLs live in the authoritative text, preserving downloads
and edited notices without introducing a separate schema or rewriting existing notices.

Both renderers link only standalone HTTPS lines and escape all link attributes and text.
No raw HTML or script URL support was added. Regression tests cover escaping, link
rendering, and removal of the media references. Graft remains unavailable.

## 2026-11-24 — Single Informed Consent template

Merged the two adapted study-information and data-privacy consent templates into one
required item named Informed Consent (`informed_consent_v1`). Preserve every section
and combine participation/data-processing agreement into a single final declaration.
Remove both superseded selector options and the user-facing provenance description;
retain Apache-2.0 licensing and adaptation attribution under third-party. Update the
user guide to the single-template workflow. Existing configured items and recorded
consent evidence remain intact; adding the new template is explicit, with no data
migration or silent alteration of participant agreements.

Validation: dashboard JavaScript checks passed. Evaluated the template definitions to
verify all twelve substantive headings, both agreements, one declaration, one required
item, and removal of the old selector definitions. Graft remains unavailable; the
existing adaptation and third-party attribution identified the two source pages.

## 2026-11-24 — Imprint subsection layout

Replace the imprint fieldset/legend box with an unboxed field container. Render Imprint
as an h4 subsection under Participant access and consent, after its general/consent
fields, in the left configuration column. Reuse the parent grid through CSS subgrid
so fields align with other settings and stack on the existing mobile breakpoint.
The editor binding, preview, and persistence contracts are unchanged. A generic
subsections list keeps layout decisions in the configuration section definitions.
Validation: all dashboard JavaScript test files passed. Graft remains unavailable.

## 2026-11-24 — Consent subsection and short field-column dividers

Move Consent items into the same subsection structure as Imprint, with its heading
in the left column and editor on the right. Omit the redundant right-column heading.
Add a grey one-pixel vertical divider inside each subsection's field column, inset
8px from its top and bottom and separated from fields by 20px. The divider is local
to that subsection rather than extending across the section or through headings;
the same containment works when columns stack on narrow screens. Consent behavior
and persistence are unchanged. All dashboard JavaScript tests passed. Graft remains
unavailable, so existing known layout definitions were inspected directly.

## 2026-11-24 — Horizontal subsection dividers

Correct the preceding divider orientation per the user's clarification: use horizontal
grey rules in the gap above Consent items and Imprint fields, spanning only their
field column. Remove the vertical rule and left padding so fields retain their shared
alignment. Heading placement and responsive stacking remain unchanged.

## 2026-11-24 — Thirteen independent informed-consent forms

Replace the combined template definition with an ordered array of thirteen items,
one per substantive heading including the final declaration. Preserve all prose and
the introduction, placed in the first item. Selecting Informed Consent adds the full
set as separately editable forms with stable semantic identifiers and independent
required/optional settings. Generalize insertion to accept either one item or a batch;
hydrate before modifying the editor and skip existing IDs to prevent duplicate forms.
Saved combined forms and past evidence are unchanged; no migration or compatibility
branch is needed. This supersedes the earlier single-form template decision.

Validation: dashboard JavaScript tests passed. Executed the actual insertion handler
with an editor fixture to verify thirteen ordered forms, unique IDs, declaration last,
repeat-add protection, and restoration of a missing section. Graft remains unavailable.

## 2026-11-24 — Visible consent-section checklist and generation

Replace the consent dropdown with an always-visible, initially unchecked list of all
17 individual options (13 informed-consent sections and four existing alternatives),
each with a short purpose description. Generate consent form replaces Add template
and the redundant Blank item control. Selected options generate separate editable
items in reading order; no selection creates one blank item with an unused identifier.
Keep prior forms and skip already-present identifiers. Hydrate the entire selected
batch before inserting anything, preserving atomic failure when institution data is
missing. Empty forms are editor drafts: existing server validation still requires
a title and statement before saving. No persistence or admission contract changes.

Validation: four new tests exercise actual renderer/generator units for all visible
options, selected-only generation, duplicate avoidance, empty generation, and hydration
failure. All six dashboard JavaScript test files passed. Graft remains unavailable.

## 2026-11-24 — One generated Consent Form and acceptance decision

Generation now joins selected section titles and hydrated bodies in reading order
into one item titled Consent Form. Use one unused identifier and one required flag
for the entire statement. No selection creates that same item with an empty body.
Existing forms remain untouched; generation appends rather than merging previously
recorded or customized consent items. Remove per-section duplicate filtering, since
section IDs no longer identify generated forms. No schema/admission changes.

Validation: focused generation tests verify one item, ordered headings, selected-only
content, unused ID, and exactly one identifier/title/statement/required control in the
actual item renderer. Empty-selection and atomic hydration-failure cases pass, as do
all six dashboard JavaScript test files. Graft remains unavailable.

## 2026-11-24 — Conductor-written section text

Each consent-section checkbox reveals an initially empty, labeled textarea directly
below the option. Hide rather than clear it on deselection. Generate Consent Form
joins only checked headings and their exact entered text in catalogue order, producing
one item and one acceptance requirement. Remove unused template body definitions and
hydration logic: default prose and institution-dependent generation are no longer part
of this workflow. Section buffers remain temporary editor state; generated statements
are saved through the existing revision mechanism. No schema changes.

Validation: five focused tests cover visibility, empty initial inputs, user text and
line-break preservation, selected-only assembly, no-selection generation, and a single
acceptance control. All six dashboard JavaScript test files passed. Graft unavailable.

## 2026-11-24 — Restore editable consent template defaults

Restore the previously used adapted Wuhn prose/placeholders for the thirteen informed
consent sections and the prior wording for the four additional Parlando sections.
Prefill each textarea once when rendering the editor, escaping its contents. Checkbox
toggling changes visibility only; generation still uses the current textarea value,
including a deliberately cleared value. No-selection generation remains empty. Use
editable institution/version placeholders in the supplemental sections rather than
blocking initial form rendering when shared configuration is missing. This restores
Parlando's prior adaptation, not upstream institution-specific claims verbatim.

Validation: all six dashboard JavaScript test files passed. Focused checks confirm
all seventeen fields contain their defaults, generation respects authored/cleared text,
and toggling preserves edits. No schema change. Graft remains unavailable.

## 2026-11-24 — Generated consent form heading

Rename generated card headings from Consent item to Consent form, retaining numbering.
Update both initial rendering and renumbering after removal so the label stays consistent.
No data or behavior change. Focused consent-generation checks passed; graft unavailable.

## 2026-11-24 — Separate collected-information sections

Remove the opening reading instruction from the study-subject default. Replace the
single collected-information option with independently selectable non-personal and
personal information options, each with its own identifier, description, and editable
prompt. Prompts distinguish anonymous/unlinkable information from identifiable or
pseudonymized participant information without assuming all study responses are anonymous.
The catalogue now has 14 informed-consent sections and four supplementary options.
Existing generated statements remain unchanged. Dashboard JavaScript checks passed;
graft remains unavailable.

## 2026-11-24 — Select all consent sections and supplied collection wording

Add Select all above the consent checklist. It checks all 18 options and reveals
all text fields without resetting edits or generating a form implicitly. Replace
personal/non-personal collection defaults with the exact study wording supplied by
the user (voice and Prolific ID; game interactions and score). These are editable
study declarations, not changes to data handling or automated privacy classifications.
Existing generated statements remain unchanged. Updated user documentation.
Validation: focused consent tests and all dashboard JavaScript test files passed,
including bulk-selection visibility and edit preservation. Graft remains unavailable.

## Deployment dashboard at the administrator origin root

The deployment host serves setup and sign-in at `/` on the configured administrator origin. The CLI prints this address and uses the same page for device approval and game sign-in redirects. Game workspaces keep their game-ID paths. Custom single-game servers keep their separate dashboard entry because their participant client can occupy the root. Old deployment dashboard bookmarks must use the root URL.

## 2026-11-23 · Agent callback deadlines and participant observations

Remote agent callbacks previously had a second five-second timer inside the gRPC adapter. A headless run could fail a model response even when the experiment explicitly allowed more time. The live runtime and headless runner now enforce their own callback and shutdown budgets. A task-local absolute deadline lets the gRPC interceptor forward only the remaining budget, including time already spent connecting or making an earlier RPC for the same callback. This preserves the public agent/factory interfaces. Calls made without a caller deadline, including learner updates, impose no transport deadline. Future operations that require a deadline should set it at the operation owner.

Issue #2 concerned the retired stateChanged.events surface. The current contract sends each role its own complete observation, including on the terminal transition. The two-browser contract test checks both actual recipient payloads before the terminal snapshots. Reconciliation restores the current role view. A game that needs a durable visible activity history can include that history in the view.

## 2026-11-23 · Bounded dashboard timeline reads

Session detail and every live poll previously materialized all durable events, including recovery checkpoints. The monitor now pages on the session's durable event index, groups only the current window of 100 raw records, and fetches alignment and successful synthesis records for its conversation messages. The raw event index provides a stable cursor even when the window contains hidden diagnostics. Older/newer controls navigate the history. Following the latest page uses the indexed high-water mark to skip unchanged polls. The monitor omits checkpoint blobs; replay continues to use the durable records. A bundle whose steps cross a page boundary is shown separately on each affected page. Speech alignment and post-terminal classification retain their session-wide meaning.

## 2026-11-23 · Uncapped corpus downloads from a consistent snapshot

A downloaded corpus is one consistent read across the eligible sessions, participants, events, and alignments. Each export from a file-backed database uses its own read-only WAL connection. The reader streams the participant registry and semantic events, keysets through sessions, and indexes the current session’s alignments in a temporary SQLite table. All three formats redact each projected record before serialization. A private temporary file allows validation to finish before the HTTP response is sent and releases the read snapshot before a slow client consumes the body. Temporary disk use grows with the download size, and the WAL can grow while preparation is in progress. The explicit in-memory SQLite mode keeps its connection and holds it through preparation. The normal server concurrency policy applies without an export-specific cap. The export route allows preparations that exceed the ordinary HTTP deadline.

In an isolated HTTP fixture, two concurrent 120,000-event downloads completed at 56.09 MiB each. The server completed 202 concurrent administrative writes (median 9.09 ms). Sampled server RSS during the pair peaked at 57.3 MiB from a 53.4 MiB pre-request baseline on this machine. These measurements describe the fixture and sampling interval, not an allocation ceiling.

## 2026-11-23 · Speech transcription readiness during room formation

Required browser transcription is ready only when the provider acknowledges recognition. A ten-second audio startup budget now covers the acknowledgement as well as connecting to the provider. Provider rejection and a missing acknowledgement produce distinct participant-facing reasons and an actor-attributed event in the session log. Failure of the current stream during formation ends the room through the existing technical-failure outcome, subject to any previously due session expiry. Replacing a socket invalidates its readiness and prevents an obsolete stream’s outcome from ending its successor. Voice relay remains available independently during setup. After activation, provider loss updates the audio status and the existing game can continue. Mock-provider tests cover a concurrent healthy pair, explicit rejection, missing acknowledgement and replacement.

## 2026-11-23 · Dashboard session search, navigation, and log display

Researchers following a Prolific participant can submit a literal suffix to the authenticated session search for their selected experiment. A POST body keeps entered identifiers out of request URLs. Filtering happens in SQLite before pagination, uses retained verified submission records, and returns ordinary session summaries. Suffix collisions can yield multiple sessions. The same lifecycle filter applies with or without a search.

The session timeline uses consistent role colors and left-aligned entries across conversation, actions, and system records. Messages-only visibility is independent of the utterance/token presentation. Workspace navigation groups the frequently used session-monitoring views together. Prolific guidance identifies the study ID by the segment following /studies/ and explains that validation checks the submitted codes and assigned actions. Provider display labels remain researcher-defined.

## 2026-11-23 · Synchronize the speech stress driver with recognizer readiness on reconnect

The speech human–agent fixture emits a transcript after 100 microphone frames reach the mock recognizer. On reconnect, the stress driver counted 100 frames sent while only 99 reached the new recognizer. It then paused microphone input while waiting for the agent to speak. Delaying the reply check still left the recognizer one frame short.

Harness revision 8 waits for a positive transcription readiness control on the replacement audio socket before resuming measured microphone input. Frame pacing restarts at that point so time spent waiting does not produce a burst of frames. This measures the agent reply to a complete fixture utterance and leaves provider startup latency outside that utterance. Speech captured by a real browser during recognizer startup is a separate behavior that this workload does not measure.

## 2026-11-23 · Two-command release workflow

Once versions and credentials are prepared, maintainers run `make test-release-candidate` and `make publish`. Both phases and the Python partial-release targets reuse a persistent installation of build and Twine. Make selects the existing project-local release environment when present or uses the configured Python interpreter. Metadata checks, Cargo dry runs and saved-bundle verification use that same interpreter. The candidate passes it to nested Python tests and end-to-end reporting so an older interpreter earlier on the maintainer’s PATH cannot take over midway. Tool installation belongs to the maintainer. The candidate target runs the optimized server build, manual build and registry dry runs before saving a verified bundle. The client build already runs in the required test matrix.

## 2026-11-23 · Independent release testing and publication

The package rehearsal stages the checkout and tests disposable archives. Publication builds each package anew from the selected checkout, including individual partial-release targets. Test and publication can run on different machines. Maintainers select the tested commit for publication; rebuilt archives can have different timestamps and hashes. Phony Make prerequisites build the npm tarball and Python distributions for both workflows. They stage clean inputs and replace the versioned outputs in target/release-packages after successful builds. The rehearsal consumes those files as a downstream client. The root Cargo alias remains available. First-party Rust consumers already reference the server by path, so they need no registry patch for it.

## 2026-11-23 · Release output and registry interaction

The two top-level release targets name their steps in Make and use the same reporter. Each invocation owns a timestamped log under target/release-logs. The reporter joins standard output and error, times each step, and stops at the first failed Make target. The last 20 lines come from that step alone. Publication runs each package separately so successful uploads and the failed retry target are visible. When the caller has a terminal the registry client receives a pseudoterminal, so a prompt without a newline can still accept input. Direct package targets and ordinary development commands retain their native output.

### Name the Python distributions for the APIs they provide

Starting with the release after 0.7.0, publish the Python game SDK as `parlando-game-sdk` and the gRPC agent SDK as `parlando-remote-agent`. PyPI and crates.io resolve names independently. Changing distribution metadata allows existing Python imports and checkout paths to stay stable. Keep the published-version badges and 0.7.0 manual commands on the live projects until the renamed releases exist.
