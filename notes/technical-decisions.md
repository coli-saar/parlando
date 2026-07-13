# Technical Decisions

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

## 2026-07-13: Agent TTS publishing waits for PCM playback

Context: Agent TTS audio is published through a short-lived LiveKit track. The publisher previously submitted every PCM chunk, waited a fixed 250 ms, and then unpublished the track. Longer utterances could still be queued for playback when the track was torn down, causing the end of the spoken agent message to be truncated in the game.

Decision: The LiveKit audio publisher now computes the playback duration represented by the PCM chunks it submits and keeps the track alive for that duration plus a small tail before unpublishing. The duration is derived from signed 16-bit PCM byte length, channel count, and sample rate, using the same shape validations required for publishing.

Tradeoffs: The publisher task remains connected for the full utterance duration, which is slightly slower for long messages but preserves the spoken output. This keeps the fix localized to transport lifetime instead of changing TTS providers or browser playback behavior.

Follow-up risks: If LiveKit exposes a reliable playout-completion signal for native audio sources, the publisher could switch from duration-based waiting to that signal. Until then, duration-based waiting is deterministic and easy to test.

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

Decision: Updated the game-generation skill and references so agents vocalize text through the server-owned agent response path, leaving synthesis and LiveKit publishing to `parlando-server`. Also directed generated browser clients to compose SDK widgets such as `MicLevelMeter`, `TranscriptionStatusChip`, and `TranscriptionProgress` from `@coli-saar/parlando-client/react` when STT is enabled.

Tradeoffs: This keeps provider credentials and audio publishing out of generated browser/game code while still giving participants visible feedback about microphone input and ASR state. It also couples generated UI guidance to the current SDK widget names.

Follow-up risks: If the server changes how agent messages trigger TTS, or if the React voice widgets are renamed or replaced, update the skill and references together.

## 2026-07-13: Generated server binaries retain WebRTC Objective-C categories

Context: LiveKit/WebRTC on macOS can abort at runtime with an unrecognized Objective-C selector when categories from the WebRTC static archive are dropped by the final game-binary link. Existing Parlando notes and the Space Game server prove that final-link `-ObjC` fixes this class of crash.

Decision: Updated the game-generation skill and server reference to require every generated Rust server crate to include `build = "build.rs"` and a macOS-only `build.rs` that emits `cargo:rustc-link-arg-bins=-ObjC`.

Tradeoffs: This adds a tiny build script to all generated server crates, including projects that may not immediately enable voice. That is preferable to making the fix conditional, because voice/LiveKit can be enabled later by config and the flag must be present in the final binary crate.

Follow-up risks: If LiveKit/WebRTC packaging changes so `-ObjC` is no longer needed, the guidance can be relaxed. Until then, generated games should keep the final-link build script rather than relying on README instructions or environment-specific flags.

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
