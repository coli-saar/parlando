# Technical Decisions

## 2026-08-13: Administrator setup is browser-first and database-backed

Context: Requiring operators to install a separate Argon2 command-line tool, generate a PHC hash, and export multiple environment variables made ordinary local administration needlessly difficult. Parlando already owns a persistent database and a protected login surface. The intended deployment workflow assumes that the operator is the first visitor to a newly created database.

Decision: When no administrator credential exists, `/admin` redirects to `/admin/login`, which presents a first-run account-creation form. The server validates the username and a password of at least 12 characters, hashes the password with Argon2id, and atomically inserts one singleton credential row containing only the username, PHC hash, administrator role, and creation time. The successful setup request receives a normal authenticated session. Subsequent setup requests fail, and future visits receive the login form. Setup and login use the dashboard's server-rendered visual language and remain usable without the game client bundle. This first-visitor setup is available wherever the server is bound, by explicit product choice. Environment credentials remain a runtime-only override for automated deployment and recovery.

Tradeoffs: First-run operation is simple and credentials persist across server restarts and binary reinstalls. A newly exposed database has a deliberate claim window until its operator completes setup, so deployment instructions require doing so before distributing the URL. Each SQLite database has an independent administrator, which means the Space Game's normal and solo-voice local databases each require setup once. Reinstalling the executable does not reset a database-backed credential.

Follow-up risks: A deployment model in which URLs become publicly reachable before operators can visit them should add an explicit provisioning or invitation mechanism. Password reset and administrator rotation are not yet exposed in the UI; the environment override is the current recovery path.

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

Decision: Implement the high-severity remediation as three explicit trust planes. Administrative routes use application-owned authenticated sessions, roles, and CSRF protection. Participants receive a non-secret public identifier plus a separate opaque credential; authenticated HTTP calls resolve the participant principal from that credential, while game and audio WebSockets use short-lived one-use purpose-bound tickets. Runtime provider, administrator, ticket-signing, and remote-agent secrets are loaded into non-serializable secret types and are never included in persistable experiment configuration or export. The complete phased design and release gates are recorded in `notes/security-remediation-plan.md`.

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

Decision: Keep `ParlandoStartupGate` focused on participant setup, consent, voice preflight, and waiting-room readiness. Do not add a generic Room ID selector, Room ID prop, or URL-query room selection behavior to the reusable startup gate. Human-human default entry is paired on the server: `POST /api/rooms` fills an existing compatible waiting room before creating a new Player A room. Study-specific code can still call `ExperimentApiClient.joinRoom` directly when it intentionally owns a room-sharing workflow.

Tradeoffs: The SDK remains less convenient for ad hoc manual joining, but generated clients avoid exposing an implementation detail to participants. Server-side first-open-room pairing is intentionally simple; future studies that need cohorts, treatments, counterbalancing, or private invite links should add an explicit pairing policy rather than putting room ids into the generic startup UI.

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

Decision: Added `docs/datenschutz-code-roadmap.md` as a separate, deliberately small implementation roadmap for fictitious two-person game studies. It is limited to versioning the displayed information/consent text, minimizing identity and voice-diagnostic data, four storage switches that leave the existing message/event schema unchanged, fixed `research` and publication-oriented `corpus` exports for the default reuse workflow, manual participant deletion in the admin UI, narrow checks around hosted speech services, and a privacy status page generated from the running version and configuration. The privacy status is installation-wide, so it gets its own protected `/admin/privacy` route linked from the dashboard's global header rather than an experiment-scoped workspace tab. Remote agents are trusted experiment code by default and require no separate privacy machinery. Automatic deletion and configurable retention jobs are explicitly out of scope. The features use ordinary feature tests; no separate privacy or compliance test suite is required. Security work remains in `notes/security-remediation-plan.md`.

Tradeoffs: The reduced scope fits Parlando's actual experiments and is implemented in the normal workflow. Structural reduction removes system identifiers but cannot prove that free dialogue contains no accidentally disclosed real-world detail, so a short content review remains a release step for a public corpus. The design does not attempt to cover clinical studies, intentional collection of special-category data, complex longitudinal identity linkage, or multi-purpose institutional data management.

Follow-up risks: Keep the small roadmap, privacy contract version, and status report aligned with schema, configuration, export, audio, and agent changes. Do not expand them speculatively; add machinery only for a concrete approved experiment that cannot be supported by the six listed changes.

## 2026-08-13: Concise privacy contract implemented in the normal research workflow

Context: The platform assessment needs executable, inspectable behavior without turning Parlando into a general compliance system. The agreed scope is self-hosted university research with fictitious two-person games, default scientific reuse, trusted experiment agents, no automatic deletion, and no change to the existing message/event representation.

Decision: Add Privacy Contract version `1`, four persistence switches, versioned participant-information evidence with a server-computed presentation hash, experiment-specific random participant identifiers, durable dialogue identifiers, and server-side minimization of voice diagnostics. Provide fixed `research`, `corpus`, and `full` admin exports; the dashboard defaults to `research`, while `corpus` is explicitly a content-review-required candidate rather than an anonymous export. Add counted, confirmed manual deletion to human participant cards. Deletion removes consent plus authored messages/transcripts, clears identity fields and the participant identifier, and removes participant references from remaining shared events. The installation-wide privacy status reports the effective configuration and capabilities but never infers DPO approval. Participant identities are scoped to the experiment, and no automatic retention job is introduced.

Tradeoffs: Random experiment-specific participant identifiers require one nullable participants-table column. Research and corpus exports use explicit projections, so new internal fields do not appear automatically. Corpus export removes internal identifiers and absolute timestamps but cannot detect identifying content typed or spoken by a participant; publication still requires removal of mappings and content review. Deleting authored action rows would damage the other participant's fictitious shared game, so those rows remain with a `deleted_participant` actor and redacted runtime identifiers.

Follow-up risks: The DSB must still assess the export allowlists, deletion boundary, local participant text, speech-provider flow, and the institution's handling of backups and already released anonymous corpora. Increment the Privacy Contract version when these technical behaviors change. Keep schema migration checks compatible with existing SQLite installations.

## 2026-08-14: Consistent readable identifiers replace participant display names

Context: Researchers need to recognize participants and dialogues in the admin dashboard and to join repeated exports of the same experiment. Export-specific random strings make that unnecessarily difficult. Participant-chosen display names also create an avoidable path for entering real names.

Decision: Assign a three-word random identifier when each experiment participant or dialogue is created and persist it in SQLite. Participant identity rows and recruitment mappings are scoped to one experiment: the same external recruitment identifier produces an independently generated participant identifier in every experiment, while repeated sessions and exports within that experiment reuse it. Participant identifiers use animal nouns; dialogue identifiers use a disjoint list of place and object nouns, so the identifier kind remains recognizable without `participant-` or `dialogue-` prefixes. Use a small local generator with `rand` and repository-owned word lists instead of adding a name-generator crate: the format is simple, and a local list makes vocabulary review and compatibility explicit. Enforce uniqueness in storage and retry a newly generated name on collision. Split legacy participant rows shared by multiple experiments, migrate legacy `research_*` participant identifiers and sessions without a dialogue identifier. Remove the participant display-name field from the browser flow, protocol, runtime state, database, exports, and admin UI. Retain the readable identifiers unchanged in both `research` and `corpus` exports of the same experiment.

Tradeoffs: The participant identifiers form part of a pseudonymization while an experiment's recruitment mapping exists; they are not proof of anonymization. Their readability does not make them less random than opaque strings drawn from an equally large space, but retaining them across exports deliberately makes records from the same participant or dialogue within that experiment joinable. They do not support joining a person across experiments. The current lists provide a finite namespace, so uniqueness still depends on database constraints and retry logic. Changing the lists affects only future identifiers; persisted identifiers remain unchanged.

Follow-up risks: A public `corpus_candidate` still needs the documented content and linkage review before it can be described as anonymous. In particular, an institution that retains a recruitment mapping can still relate a published participant label to its internal participant row. Vocabulary changes should exclude ambiguous, offensive, identifying, or easily confused words and keep the two noun sets disjoint.
