# Technical Decisions

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

Decision: Keep `Game` as the complete shared contract for authoritative mechanics and do not add a state-owning `Session` domain object. A `SessionRunner<G>` owns local `Game::State` while it executes one headless session. It uses a small Rust `AgentSchedule<G>` strategy to select the first and subsequent agents. The versioned YAML experiment file selects a compiled schedule by stable kind and ordinary constructor parameters; it does not encode a scheduling transition language. `AgentSchedule::next` receives the existing optional `AgentResponse<G::Action>` plus an optional `ActionRejection`; do not add a near-duplicate `DecisionOutcome` representation. Scheduling controls only when software agents are asked to respond; `Game::apply_action` remains the sole authority on action legality. Runner-owned timeouts, deadlines, decision and message limits, invalid-action limits, and unconditional shutdown remain outside the schedule. `AgentSchedule` is headless-only: issue #22 leaves the live runtime unchanged. The detailed design and examples live in `notes/agent-agent-runner-design.md`.

Tradeoffs: The new headless driver is not a universal orchestration abstraction, which keeps the agent-agent heartbeat easy to understand and test and avoids coupling issue #22 to live execution. Some calculation around accepted transitions may initially be duplicated; extract a stateless helper only when concrete duplication warrants it. Adding a new schedule kind requires Rust code and registration, while its selection and small parameters remain convenient in YAML. This avoids maintaining a public scheduling DSL. Reusing `AgentResponse` makes the schedule generic over `G` and exposes typed action and message contents to specialized scheduling code; built-in schedules can ignore those payloads, and this cost is preferable to maintaining a second response classification with invalid representable combinations.

Follow-up risks: The headless driver must conform to the existing public `Game` and `Agent` contracts for callback ordering, completion delivery, and cleanup. Its terminal behavior for `ActionAndMessage` must be tested explicitly; any change to live behavior is separate work. Result artifacts must identify the schedule implementation, version, and parameters for reproducibility. A schedule that repeatedly selects the same agent can fail to make progress, so hard runner limits must remain authoritative and independent of scheduling code.

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

Decision: Make a versioned, strictly validated YAML file the public specification for an agent-agent experiment. Named agents are defined separately and assigned independently to the two seats for each phase; scenario lists contain game configurations, seeds, and repetitions. `ExperimentRunner` expands that file into deterministic `SessionPlan` values, runs `SessionRunner` with bounded concurrency, finalizes artifacts, and, in RL mode, owns the epoch, checkpoint-update, and held-out-validation cadence. Do not add `SessionRecorder`; bounded session results carry optional traces or trajectories, with an internal streaming sink left as a performance substitution. `RLAgent<G>` owns observation and action encoding, inference, optimization, and resolution and persistence of opaque semantically immutable checkpoint IDs. Keep reward outside both `Game` and `RLAgent`: a selected compiled `RewardFunction<G>` may inspect authoritative transitions inside `SessionRunner` but releases only per-role numeric rewards. The initial RL path treats accepted actions as environment steps and supports action-only learning agents. The full design lives in `notes/agent-agent-runner-design.md`.

Tradeoffs: The YAML schema becomes a compatibility surface that requires versioning, strict validation, and migration discipline. Per-session trajectory buffering is simple and bounded by hard limits but may later need an internal streaming sink. Opaque checkpoint IDs allow in-memory, file-backed, or remote learners and keep save cadence inside `RLAgent`; cross-process resume is possible only when that implementation can resolve a recorded ID. Keeping rewards in a separate trait permits multiple learning objectives per game but requires the experiment to select and record one. Action-only RL postpones communication-policy step and encoding semantics.

Follow-up risks: Canonical YAML normalization, session identity schemas, status codes, shard sizing, training-update idempotency, and the first RL validation game remain implementation choices. Version 1 requires `epochs` to be divisible by `checkpoint_every_epochs`, avoiding an implicit final partial update. Partial shards must never be treated as finalized, remote retries must not duplicate completed logical callbacks, and cleanup must release agents and provider permits on every exit path. The headless driver needs contract tests for accepted transitions, observations, completion, and terminal combined-response behavior; no live runtime change is part of issue #22.

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
