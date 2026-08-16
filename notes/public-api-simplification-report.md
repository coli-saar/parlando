# Public API Simplification Report

**Status:** Implemented API decision and post-implementation review.

**Date:** 2026-08-16

## Executive summary

Parlando's public interfaces currently expose more of the runtime's implementation structure than game authors need. The largest leaks are the complete experiment configuration tree, provider-specific audio and transcription types, storage and router machinery, low-level browser audio classes, and wire-protocol data transfer objects. Each exposed name creates a compatibility obligation even when its only consumers are Parlando's own dashboard or tests.

The recommended public boundary is task-oriented. A downstream author should need public API only to:

1. implement game mechanics;
2. start a game server;
3. register an optional agent implementation;
4. build a participant application; or
5. build a custom participant client when the React application is insufficient.

The experimenter dashboard and durable game logs add two legitimate extension needs. A compiled game must be able to contribute game metadata, validate game-owned configuration, describe compiled agents, and attach structured metadata to durable game transitions. These needs do not require exposing dashboard handlers, database records, provider clients, or the full experiment configuration.

The implemented changes are:

- rename `GameAdapter` to `Game` and replace `GameDescriptor` with `GameMetadata`;
- replace `ServeOptions` and the family of `serve*` functions with a `Server` builder;
- make the complete experiment, provider, storage, routing, and audio implementation private;
- reduce the `Game` trait by combining validation with application and completion detection with completion construction;
- replace `events_for_action` with a role-neutral durable logging hook;
- register agent factories directly with the server and let each factory own its dashboard definition;
- rename `ParlandoStartupGate` to `ParticipantApp` and expose only role-safe game-session data;
- retain one managed `ParticipantClient` for nonstandard participant applications, but hide raw sockets, credentials, diagnostics, and server-message DTOs behind narrow one-use transport plans; and
- use Game, Experiment, and Session consistently in code and in the dashboard, reserving study for the broader research project that Parlando does not model directly.

The tables below contain only names that should change or cease to be public. Unchanged public names are omitted.

## 1. Scope and decision criterion

Rust's `pub` and a JavaScript package entry point serve a different purpose from an internal module boundary. Dashboard contributors can use `pub(crate)` Rust items and non-entrypoint TypeScript exports without committing Parlando to downstream compatibility. Integration tests can be moved inside the crate or into an explicitly private test-support crate. Neither case establishes a reason for a production-public type.

A type or operation should remain downstream-public only if at least one supported external task requires it. A hypothetical future integration is not sufficient: adding a well-designed extension later is generally non-breaking, whereas removing an exposed type requires a migration. This criterion still permits generalization, but places extension points at domain boundaries rather than at incidental implementation seams.

The current public Rust surface begins in [`rust-server/src/lib.rs`](../rust-server/src/lib.rs). Because several implementation modules are declared with `pub mod`, their public contents are reachable even when they are not re-exported from the crate root. The JavaScript package has only `.` and `./react` entry points in [`js-client/package.json`](../js-client/package.json), so symbols exported from source files but absent from these entry points are not package-public.

## 2. Domain terminology

The dashboard already establishes the useful hierarchy:

- a **game** is the compiled mechanics and participant application;
- an **experiment** is one versioned configuration and data-collection context for that game; and
- a **session** is one play-through within an experiment.

A **study** is normally the broader research project. It may contain several Parlando experiments, use several games, or include work outside Parlando. Because Parlando does not model this entity, using study as a synonym for experiment creates a false hierarchy.

| Current | Proposed | Rationale |
|---|---|---|
| `StudyConfig` | Private `SessionConfig` | The remaining values are runtime-owned session lifecycle policy; game authors and participant applications do not configure them. |
| `study.name`, “Study name”, and proposed participant-title fields | Delete | The unique experiment ID is the researcher-facing catalogue identity, and participant startup uses the compiled `GameMetadata.name`. A title-only configuration object has no domain responsibility. |
| “this study” in participant UI | “this experiment” | The participant enters a dashboard-managed experiment. |
| `DirectConfig` | Private `ParticipantAccessConfig` | “Direct” describes a routing mechanism; the contents concern access, information, and consent. |
| Proposed `ParticipantIntakeConfig` | Do not introduce | Intake would also imply lifecycle state, readiness, capacity, and pairing. The current object represents only participant access and consent. |
| `completionSummary` | `completion` | Matches the dashboard and stored session terminology. |

`ParticipantApp` is preferable to `BrowserApp`. Both the participant application and the experimenter dashboard run in a browser, so `BrowserApp` does not distinguish their responsibilities. `ParticipantApp` does.

## 3. Implemented Rust surface

The ordinary crate root should be close to four concepts: the game contract, game metadata, player role, and server. Optional agent support should live in an `agent` module. Built-in runtime facilities should remain private until a concrete external implementation demonstrates the need for a supported extension API.

### 3.1 Types and module exposure

| Current | Proposed | Rationale |
|---|---|---|
| `pub mod config`, `audio`, `audio_publisher`, `transcription`, `tts`, and `remote_agent` | Private modules with selective re-exports | Public modules make all their public implementation types downstream contracts. |
| `GameAdapter` | `Game` | This is Parlando's game contract, not an adapter around a separately public game abstraction. |
| Possible `BuiltinGame` | Do not introduce | A TextArena-backed or remote implementation can implement `Game`; the trait need not encode a hypothetical taxonomy. |
| `GameDescriptor` | `GameMetadata` | The value contains identity, display, version, and build metadata rather than behavior. |
| `GameDescriptor.display_name` | `name` | The containing type already supplies the display context. |
| Public `GameDescriptor::validate()` | Validate inside `Server::new` | Construction should establish validity instead of requiring a second caller action. |
| `Seat` | Private or delete; use `PlayerRole` at the public boundary | `Seat` and `PlayerRole` represent the same A/B distinction at different implementation layers. |
| `ExperimentConfig` and all nested configuration types | Private | The tree combines bootstrap, dashboard, privacy, pairing, provider, lifecycle, and game settings. A game needs only its game-owned configuration. |
| `SpeechmaticsConfig`, `TranscriptionConfig`, `TtsConfig`, and `VoiceConfig` | Private | These configure built-in runtime services, not game mechanics. |
| `AgentsMode`, `HumanVsAgentConfig`, `CapacityConfig`, and `PrivacyConfig` | Private | These are Parlando policy and dashboard persistence types. |
| `ServeOptions` | `Server` builder with private fields | A builder can expose supported composition operations without publishing a growing bag of internal hooks. |
| `AgentFactoryDescriptor` | `agent::Definition`, owned by the corresponding factory | The dashboard catalogue is a valid extension, but its metadata should not be separate from the factory that consumes its settings. |
| `AgentConfigFieldDescriptor` | `agent::ConfigField` | Retain a concise typed field description because custom compiled agents contribute structured settings to the dashboard. |
| `AgentParticipantIdentity` | `agent::Identity` with semantic `name` and `version` fields | `identity_provider`, `external_id`, and arbitrary storage metadata expose the database representation. |
| `AgentInitContext` | `agent::Context` | The module supplies the agent qualifier. |
| `AgentInitContext.role: String` | `role: PlayerRole` | In-process Rust code should use the domain type rather than a wire-format string. |
| `AgentInitContext.config` | `agent::Context.settings` | Distinguishes agent-owned settings from the complete experiment configuration. |
| `GameAgent` | `agent::Agent` | The generic game parameter already supplies the game context. |
| `AgentFactory` | `agent::Factory` | Namespacing permits the shorter name without ambiguity. |
| `AgentUtteranceKind` | Delete | Messages have a sender and text; transport modality is not an agent callback distinction. |
| `SharedAgentFactory` | Private or delete | `Arc<dyn Factory<_>>` is an ownership choice rather than a domain concept. |
| `RemoteGrpcAgentConfig` | Delete from the public API | The registered gRPC factory can interpret experiment-selected agent settings. |
| `RemoteGrpcAgentFactory` | `agent::grpc::Factory` | One public integration object is sufficient. |
| `RemoteGrpcAgent` and `remote_agent::pb` | Private | The per-session client and generated protocol types are implementation details. |
| `AgentAudioPublisher` and `AudioPublishSummary` | Private | Their present use is runtime composition and testing, not a supported game-author task. |
| `StreamingTtsProvider`, `TranscriptionProvider`, and provider DTOs | Private | A future provider API should be designed around a concrete external implementation rather than the current built-ins. |
| `merge_sqlite_catalogues`, `CatalogueMergeReport`, and `CatalogueRowCounts` | Remove | The one-time local consolidation is complete, and database-import machinery is not part of the reusable runtime. |
| Public storage records, `ExperimentStore`, `SqliteExperimentStore`, `AppState`, and `AppError` reachable through modules | `pub(crate)` | The dashboard and server can use them without imposing downstream compatibility. |

### 3.2 `Game` trait

The former [`GameAdapter`](../rust-server/src/game.rs) trait exposes duplicate operations and one semantically inconsistent event hook. The implemented contract should make the secure operation the easiest operation: a game applies an action for a specified actor, produces a role-specific observation for a specified viewer, and optionally contributes role-neutral metadata to the durable log.

| Current | Proposed | Rationale |
|---|---|---|
| `type Event` | Delete | It currently conflates participant presentation with durable transition logging. |
| `type Summary` | `type Completion` | The value exists only when a session completes. |
| `initial_state()` plus `initial_state_with_config(config)` | `initial_state(config, seed) -> Result<State>` | One initialization path prevents game configuration from being silently ignored. |
| `validate_config(config)` | Retain with parameter `config` | Dashboard validation is a legitimate public extension; the parameter should state its limited scope. |
| `agent_factories()` | Delete from `Game`; register factories on `Server` | Game mechanics should not own process composition. |
| `parse_action(action: Value)` | Delete | `Action: DeserializeOwned` already defines parsing; special representations belong in `Deserialize`. |
| `validate_action(state, action, player)` plus `apply_action(state, action)` | `apply_action(state, action, actor) -> Result<State>` | Authorization and transition cannot drift or be called in the wrong order. |
| `observe_state(state, player)` | `observation(state, role)` | The method is a pure, role-safe projection rather than an observation side effect. |
| Proposed `observation_for(state, player)` | `observation(state, role)` | The explicit `role` parameter carries the important relation; `_for` adds little. |
| `available_actions(state, player)` | `available_actions(state, role)` | The result is a viewer-specific affordance, not an authoritative legality oracle. |
| `events_for_action(before, after, action, player)` | `transition_metadata(before, after, action, actor) -> Option<Value>` | The replacement contributes structured, role-neutral metadata to dashboard logs and exports. |
| `is_complete(state)` plus `completion_summary(state)` | `completion(state) -> Option<Completion>` | One method removes the invariant that two independent completion methods must agree. |

The `Option<Vec<Action>>` result of `available_actions` remains useful: `None` means that the game does not provide an action catalogue, whereas `Some(vec![])` means that it provides one and no action is currently available. Submitted actions must still pass through `apply_action` and must not be trusted merely because they appeared in this list.

### 3.3 Durable game logs

The former `events_for_action` contract does not match its implementation. In [`submit_action`](../rust-server/src/app.rs), the server calls the method once with the acting role and persists the returned value inside `game_action_accepted`. In [`broadcast_player_views`](../rust-server/src/app.rs), the server sends an empty event list to each participant. The value is therefore neither a canonical role-neutral log entry nor a per-viewer participant event.

Parlando should own the durable transition record. It records the actor, submitted action, event ordering, timestamps, completion, and—when the experiment's privacy settings permit it—a state snapshot. Human players and agents receive the accepted action together with their own role-specific resulting observation. This gives both controller types the same domain information without exposing the authoritative state. A game that needs an uninformative public cause can represent that explicitly in its action type, for example with a `SecretAction` variant. The game may add `transition_metadata`, which must be structured and role-neutral because it becomes part of the dashboard and export record. The game must not use this hook for personalized strings such as “You discovered …”.

Participant-private transient events should not be retained as an accidental second meaning of the same hook. If a demonstrated game later requires them, Parlando can add a separate contract such as `participant_events(before, after, action, actor, viewer)`. Such values would be computed per recipient and would not be persisted as the canonical transition record.

This separation retains a useful game-log extension without exposing the underlying storage API or permitting game code to bypass privacy, ordering, and transactional persistence.

### 3.4 Agent methods

| Current | Proposed | Rationale |
|---|---|---|
| `observe_state(current_observation)` | `start(initial_observation)` | Construction finishes before game state is created; the initial role-specific observation begins game delivery. |
| `observe_action(actor, action, resulting_observation)` | `observe_transition(actor, action, observation)` | The shorter parameter retains the relevant meaning. |
| `maybe_act(available_actions)` plus `act(available_actions)` | `respond(available_actions) -> Option<Response>` | One decision operation is sufficient; the runtime can enforce when declining is disallowed. |
| Public `AgentResponse::is_empty()` | Private runtime validation | Agent authors construct responses but do not need the validation helper. |
| `participant_identity()` | `identity()` | Storage-oriented participant terminology is unnecessary on a factory. |
| Descriptor returned separately by `Game::agent_factories()` | `Factory::definition()` | Keeps dashboard metadata beside the implementation that consumes its settings. |
| `serve_game(..., options_factory: Fn(&ExperimentConfig) -> ServeOptions)` | `Server::agent(factory)` | Prevents exposure of the full experiment configuration and prevents catalogue/resolver drift. |

The runtime should call `respond` in both optional and required-response situations. The response enum cannot be empty; `None` means the agent declines to respond now. This policy does not need a second public trait method.

### 3.5 Server functions and parameters

| Current | Proposed | Rationale |
|---|---|---|
| `serve_game(adapter, bootstrap, descriptor, bind_addr, options_factory)` | `Server::new(game, metadata).serve(address)` | Starting a server should not require knowledge of dashboard and provider internals. |
| `ServeOptions.agent_factory` | `Server::agent(factory)` | Registers supported agent implementations directly. |
| `ServeOptions.tts_provider`, `transcription_provider`, and `audio_publisher` | Private integration hooks | These are built-in composition and testing seams without a current downstream provider ecosystem. |
| `ServeOptions.game_version_manifest` | Fold into `GameMetadata` | Game identity and provenance should have one owner. |
| `serve(...)` | Delete | It is the legacy single-experiment entry point. |
| `build_router(...)` and `build_game_router(...)` | Private | Internal tests and dashboard work do not establish a supported router-embedding API. |
| Positional `bind_addr` | Builder or serve parameter `address` | `bind_addr` describes the socket implementation rather than the caller's task. |

The server builder should expose only information needed before the dashboard can open: database location, participant application assets, registered agents, and the listening address. Experiment lifecycle, participant access, pairing, privacy, transcription behavior, synthesized voice, and game settings remain dashboard-owned.

## 4. Implemented JavaScript surface

The ordinary React consumer should use `ParticipantApp` and a role-safe `GameSession`. A custom non-React participant application can use `ParticipantClient` for the HTTP lifecycle and one-use transport plans, then implement the documented socket protocol. Neither consumer handles bearer credentials or diagnostic posts; ordinary React games also do not handle socket URLs, audio plans, or separate microphone and playback controllers.

### 4.1 Application and session API

| Current | Proposed | Rationale |
|---|---|---|
| `ParlandoStartupGate` | `ParticipantApp` | The component owns the complete participant lifecycle; “gate” understates its responsibility. |
| `ParlandoStartupGateProps` | `ParticipantAppProps` | Follows the component rename. |
| Possible `BrowserApp` | Do not introduce | The experimenter dashboard is also a browser application. |
| `ActiveParlandoSession` | `GameSession` | Concise and consistent with dashboard session terminology. |
| `GameSession.state` | Delete | Participant code should receive only its role-safe `observation`, never authoritative state as a fallback. |
| `GameSession.events` and generic `TEvent` | Delete | The server does not deliver the semantics claimed by this field. |
| `GameSession.participantSessionId` | Delete | The active game UI does not need the runtime identity. |
| `GameSession.publicConfig` | Replace with narrow properties such as `voiceEnabled` | The active game should not depend on the complete participant intake and provider response. |
| `GameSession.completionSummary` | `completion` | Matches Rust and dashboard terminology. |
| `GameSession.sendChatMessage(text)` | `sendMessage(text)` | The session context already establishes participant conversation. |
| `ParticipantApp` prop `apiClient` | Optional `baseUrl` | Deployment configuration is legitimate; injecting the internal client into every game is not. |
| `ParticipantApp` prop `createAudioController` | Delete from the public props | This is a testing and implementation seam. |

The `GameSession` role should remain a public TypeScript interface rather than a separately constructible class. `ParticipantApp` creates and manages it. This preserves a high-level React contract without adding another public constructor.

### 4.2 Managed participant client

| Current | Proposed | Rationale |
|---|---|---|
| `ExperimentApiClient` | `ParticipantClient` with a narrower managed API | It implements one participant workflow rather than experiment administration. |
| Positional `ExperimentApiClient(baseUrl)` | `ParticipantClient({ baseUrl? })` | An options object permits coherent future client options without positional growth. |
| Snake-cased `ExperimentInfo` fields | `status`, `participantInformationVersion`, and `participantInformationUrl` | The managed JavaScript API should not leak Rust wire naming. |
| `getPublicConfig()` | `getExperiment()` | Returns participant-visible experiment information rather than arbitrary configuration. |
| `createParticipant()` | `register()` | The client can retain credentials internally instead of returning transport material. |
| `submitConsent(decisions)` | `acceptConsents(decisions)` | Names the participant action directly. |
| `createRoom()` | `join()` returning `JoinedRoom` | A participant joins matchmaking; internal identities and raw response fields stay hidden. |
| `sendAction(socket, action)` | `GameSession.sendAction(action)` | Raw sockets should not appear in application code. |
| `sendChatMessage(socket, text)` | `GameSession.sendMessage(text)` | Message transport belongs to the managed session. |
| `leaveSession(socket)` | `GameSession.leave()` | Session ownership should include intentional departure. |
| `postVoiceDiagnostic` and `socketUrl` | Private | These are standard-app implementation details. Keep authenticated `getAudioSession` and `getGameSession` so a custom client can obtain narrow one-use transport plans. |
| Raw `RoomResponse`, `ServerMessage`, and `ParticipantCreateResponse` DTOs | Private protocol DTOs | Wire-format shapes should not be the ergonomic application model. |
| Wire-shaped audio and game session results | Exported `AudioSessionPlan` and `GameSessionPlan` values with camel-cased fields | A custom client needs to name these narrow results, but should not inherit raw protocol naming. |
| `apiBase`, `checkedJson`, and `socketUrl` | Private | These are transport helpers rather than domain operations. |
| `PublicConfigResponse.study_name` and proposed `participantTitle` | Replace with compiled `game_name` | Participant startup can identify the game without introducing mutable experiment presentation state. The value comes from `GameMetadata.name`, not experiment configuration. |

`ParticipantClient` is the one lower-level class worth retaining for registration, consent, matchmaking, and authenticated one-use transport plans without requiring React. `ParticipantApp` uses it internally and adds socket, reconnection, and audio coordination. A non-React frontend consumes a plan and implements the documented versioned socket protocol; publishing every raw DTO and transport helper would not create a better abstraction.

### 4.3 Audio classes, helpers, and components

| Current | Proposed | Rationale |
|---|---|---|
| `AudioSessionController`, `MicrophoneSource`, and `ParlandoAudioSink` | Private | Their independent composition exposes implementation complexity without a demonstrated downstream task. |
| `AudioSessionContext`, `AudioSessionSnapshot`, `LocalAudioSink`, `MicrophoneInput`, and `AudioSinkPurpose` | Private | These types exist to connect the internal audio classes. |
| `initialVoiceStatus` and `initialVoicePreflight` | Private | They are implementation defaults and test fixtures. |
| `MicLevelMeter` | `MicrophoneLevelMeter` | Avoids an abbreviation in a small user-facing component surface. |
| `DeviceSelect` | Private | Device selection is part of preparation owned by `ParticipantApp`. |
| `VoicePreparationControls` | Private | Same lifecycle ownership. |
| `createDefaultAudioController` and `useVoiceController` | Private | Implementation and testing hooks. |
| `VoiceStatusChip` | Delete from the public API | It is a trivial rendering of a status string and currently embeds study terminology. |
| `bothPlayersConnected`, `requiredConsentsAccepted`, and `transcriptionProgressForStatus` | Private | These are standard participant-app policy and rendering helpers. |
| `MicrophoneMuteButton` props `voiceEnabled`, `onMicrophoneMutedChange`, and `voiceStatus` | `enabled`, `onMutedChange`, and `status` | The component name supplies the repeated microphone and voice context. |
| `TranscriptionProgress` props `gameConnected` and `voiceStatus` | `connected` and `status` | Same contextual simplification. |

Source files may continue exporting some of these names to sibling modules and tests. They should cease to be exported from the package entry points in [`js-client/src/index.ts`](../js-client/src/index.ts) and [`js-client/src/react.tsx`](../js-client/src/react.tsx).

## 5. Python agent package

The Python package already has a small surface. It should use the same vocabulary as Rust while retaining its simple response value.

| Current | Proposed | Rationale |
|---|---|---|
| `GameAgent` | `Agent` | The package context already establishes that this is a Parlando game agent. |
| `serve_agent` | `serve` | The package serves only agents, so the suffix is redundant. |
| Public access to `serve_agent_async` outside `__all__` | Keep internal unless async embedding becomes supported | Module reachability should not become an accidental compatibility promise. |

Generated protobuf modules and transport helpers should remain internal. They implement the remote-agent protocol but are not the author-facing agent abstraction.

## 6. Dashboard boundary

Dashboard contribution warrants public game-supplied metadata, not public dashboard internals. The supported flow should be:

- `GameMetadata` supplies compiled game identity and provenance;
- `Game::validate_config` validates dashboard-edited game YAML;
- each registered `agent::Factory` supplies its `agent::Definition` and `agent::ConfigField` values;
- `Game::transition_metadata` enriches durable action records; and
- `Game::completion` supplies the shared, participant-visible result and the result stored for a completed session. It must be safe for both roles; private terminal facts remain in role-specific observations.

The dashboard implementation can use `pub(crate)` handlers, request objects, storage records, telemetry, authentication, and export projections. Contributors working in the Parlando crate retain full access to these internal boundaries. Downstream game crates do not need them.

Parlando does not currently support game-specific dashboard panels. If that becomes a concrete requirement, it should receive one versioned extension contract, for example a constrained `DashboardExtension`, with explicit data access and rendering rules. Exposing `AppState`, routers, or database stores now would not provide a safe substitute: it would couple extensions to authentication, schema, privacy, and routing internals.

## 7. Generalization and deliberate omissions

Making a type private does not prevent Parlando from generalizing later. Adding a new public provider or dashboard extension is non-breaking; maintaining an unsuitable existing interface is not. The proposal therefore retains extension points only where current product behavior supplies a concrete contract.

The following capabilities remain possible without public implementation classes:

- TextArena and other externally sourced games can implement `Game`.
- Custom agents can implement `agent::Agent` and `agent::Factory`.
- Agent settings can appear in the dashboard through `agent::Definition`.
- Game-specific configuration can be edited in the dashboard and validated by `Game`.
- Game-specific transition metadata can appear in logs and exports.
- Non-React participant applications can combine `ParticipantClient` transport plans with the documented protocol.
- The documented HTTP and WebSocket protocol remains available for clients that need complete transport control.

The proposal deliberately does not promise arbitrary TTS or transcription providers, custom persistence engines, custom routers, custom audio transports, or custom dashboard panels. Each would require invariants involving security, privacy, lifecycle, or transactional behavior that the current low-level types do not express.

## 8. Migration strategy

This cleanup should be released as one explicit breaking version while the downstream ecosystem is small. Long-lived aliases would preserve the old conceptual model and make the final surface harder to understand. Existing stored data and export readability should be preserved unless a separate data-model decision requires a migration.

### Phase 1: agree on contracts

1. Confirm Game, Experiment, Session, Participant, Agent, and Completion as the runtime vocabulary.
2. Confirm `ParticipantApp`, rather than `BrowserApp`, as the React component name.
3. Confirm that durable transition metadata and participant-private transient events are distinct concepts.
4. Confirm that custom providers, persistence engines, router embedding, and dashboard panels are not supported public extensions in this release.

### Phase 2: simplify Rust

1. Introduce `Game`, `GameMetadata`, and the `Server` builder.
2. Port Space Game to the reduced trait before changing visibility; this tests whether the ordinary authoring path remains convenient.
3. Replace `events_for_action` with `transition_metadata` and preserve old event payloads when reading historical exports.
4. Register agents on `Server`, with each factory supplying its dashboard definition.
5. Make implementation modules and storage types crate-private.
6. Move tests that require internals into crate unit tests or an explicitly private test-support crate.

### Phase 3: simplify JavaScript

1. Introduce `ParticipantApp` and `GameSession` and port Space Game.
2. Remove authoritative state, protocol identities, raw configuration, and the event generic from the rendered session.
3. Make `ParticipantClient` own registration, consent, joining, credentials, and authenticated plans; make `ParticipantApp` own sockets and audio coordination.
4. Remove low-level audio classes, protocol DTOs, and helpers from package entry points.
5. Retain only components that a game actually uses while rendering an active participant session.

### Phase 4: align Python and documentation

1. Rename the Python agent and serving entry point.
2. Update architecture, game-building, agent, participant-protocol, and deployment documentation.
3. Update the game-generation skill and all checked-in examples.
4. Replace study terminology in participant-facing strings while preserving study as a broader research concept in explanatory prose.

### Phase 5: verify the boundary

1. Build and test the Rust server, JavaScript package, Python package, and Space Game.
2. Run package-consumer smoke tests that import only documented entry points.
3. Add a check that records the Rust and JavaScript public surfaces so new exports are reviewed deliberately.
4. Exercise dashboard configuration, agent selection, live session logs, research export, corpus export, and historical event reading.

## 9. Migration guide requirements

The migration guide should be task-oriented rather than a flat symbol list. It should include complete before-and-after examples for:

1. implementing a game;
2. starting the server;
3. registering an in-process agent;
4. registering a gRPC agent;
5. rendering a participant game with `ParticipantApp`;
6. using `GameSession` fields and methods;
7. building a custom participant flow with `ParticipantClient`; and
8. contributing durable transition metadata.

Each section should state the behavioral change as well as the rename. In particular:

- action validation now occurs inside `apply_action`;
- initial state always receives game-owned configuration;
- completion is represented by `Option<Completion>`;
- participant code no longer receives authoritative state;
- raw sockets and participant credentials are managed internally;
- `events_for_action` data is no longer treated as both participant presentation and durable logging; and
- dashboard and provider configuration classes are no longer downstream-public.

The guide should contain a final search checklist for removed names and should identify the first release containing the new API. It should not recommend compatibility aliases as a permanent migration technique.

## 10. Resolved decisions and deferred work

The cleanup resolves the original decisions as follows:

1. The durable hook is `transition_metadata`; it is role-neutral and may inspect both states.
2. `ParticipantClient` remains as the narrow HTTP lifecycle client used by `ParticipantApp`; raw protocol DTOs and transport helpers are not package exports.
3. Provider replacement, router embedding, and participant-private transient events are not supported public extensions in this release.
4. Participant titles are deleted rather than renamed.
5. Cross-origin browser hosting remains protocol-compatible but lacks a public server allowlist; add one focused origin-policy method in future work instead of exposing server configuration.

## Conclusion

The main opportunity is to replace implementation-shaped exposure with a small set of domain contracts. Dashboard and logging needs fit within that boundary: games supply metadata, configuration validation, agent definitions, transition metadata, and completion values, while Parlando retains ownership of administration, privacy, persistence, and transport. This arrangement reduces the migration and maintenance burden without preventing future games, agents, participant applications, or deliberately designed extension modules.

## 7. Post-implementation review

The new API is materially better. The strongest improvement is semantic rather than cosmetic: one typed `Game` contract now owns the complete path from role-authorized action to new state, role-specific observation, optional role-neutral log metadata, and optional game-specific completion. Completion remains an unconstrained typed payload for shared public results such as win/loss, winner, and scores; Parlando does not impose a universal result schema. Removing public state and generic events from participant sessions eliminates two competing information channels. Separating agent construction from `start(initial_observation)` also gives expensive agents a coherent lifecycle without making game state constructor input.

The supported Rust root is now deliberately small: `Game`, `GameMetadata`, `PlayerRole`, `ActionRejection`, `Server`, and the optional `agent` namespace. Dashboard configuration, provider integrations, storage, routers, generated protobufs, and pairing policy are internal. The Space Game example likewise exports only `SpaceGame` and `BackAndForthAgentFactory`; its state engine is not a second public API, and its obsolete `SpaceEvent` was deleted. The versioned wire protocol is frontend-neutral, while the JavaScript package offers a managed client and an optional React application layer.

Remaining concerns are narrower and no longer require reopening the game model:

| Area | Concern | Recommendation |
|---|---|---|
| Agent readiness | The runtime currently treats completion of async factory creation as readiness. Synchronous work inside an async method can still occupy a runtime worker, and initialization timeout/cancellation semantics are not yet an explicit contract. | Make this the first follow-up change. Define readiness and cancellation, distinguish initialization from turn timeout, and isolate blocking model loading. Do not solve it with game messages or presentation state. |
| Cross-origin browser deployment | The protocol is frontend-neutral, but `Server` currently has no focused allowed-origin method. Same-origin assets and a shared reverse-proxy origin work today. | Add an explicit origin allowlist in a future server change; do not expose internal server configuration. |
| JavaScript client depth | `ParticipantClient` deliberately manages registration credentials and HTTP lifecycle, while `ParticipantApp` owns the full socket/audio lifecycle. A non-React client still implements the documented socket protocol. | Keep the boundary small. Introduce a managed non-React socket session only when a real consumer requires it. |
| Rust test support | `test_support` is now absent from default builds and available only behind the unsupported `internal-tools` feature for Parlando's binaries and integration tests. | A private support crate could make the physical boundary stronger during a future test reorganization; do not restore default exposure. |
| Agent settings schema | `agent::ConfigField.kind` is a string because the dashboard currently supports a small evolving control catalogue. | Keep it until a concrete nested or secret-bearing agent setting requires a typed schema; then replace it deliberately rather than generalizing speculatively. |
| Agent cleanup and first-decision ordering | A room treats a constructed agent as ready before `start` returns. A transition can queue during `start`, and failure paths can skip inbox removal or `shutdown`. | Resolve this with readiness: serialize observations before the first `respond` and make cleanup unconditional. |
| Participant status localization | Rust currently supplies English operational status and some HTTP error text. | English is acceptable now. Introduce stable status and error codes only when another frontend or localization requirement demonstrates the need. |

None of these concerns argues for restoring `GameAdapter`, public experiment configuration, events, state delivery, provider configuration, or pairing types. They are follow-up boundary refinements around an otherwise coherent domain API.

## 8. Final documentation review

Before the API cleanup is handed off, revise the user-facing README and API guides as one coherent explanation of the new model. The review should begin with the `Game`/`Observation`/`Completion` mental model, show one small complete Rust game and React participant application, explain agents after the ordinary human-facing path, and reserve raw protocol details for advanced clients. Remove old terminology rather than teaching aliases outside the migration guide.

Use [`docs/client-protocol.md`](../docs/client-protocol.md) as the user-facing wire reference and [`notes/message-protocol.md`](message-protocol.md) as the protocol design and review note. The final documentation pass is a required deliverable, not optional follow-up work.
