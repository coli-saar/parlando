# Technical Decisions

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

Follow-up risks: As Prolific automation and the planned generation skill mature, the README and docs index should be revisited so the entry path reflects the actual supported workflow rather than future intent.

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

Decision: Implement one versioned `/ws/audio/{room_id}` transport carrying fixed 20 ms PCM16 frames, authenticated by an opaque, one-use, five-minute room/participant/role token whose claims remain only in server memory. The in-process room registry relays human audio to the other role and fans the same validated frame into a bounded `TranscriptionProvider` session. Speechmatics is the first provider and runs exclusively on the server. Its streaming partial/final messages are normalized into `FinalTranscriptUtterance`; final utterances are idempotently persisted, broadcast as conversation messages, and delivered to agents as spoken observations. Agent PCM uses the same room registry without entering STT. The browser uses AudioWorklets for 24 kHz capture/resampling and playback with a configurable jitter threshold. All previous browser sinks, native server dependency, public transcript ingestion, temporary-key minting, and provider-specific browser exports are removed.

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
