# Comprehensive Test Design

## Purpose and scope

This plan covers the reusable code shipped from `rust-server` and `js-client`, including the Rust command-line utilities and the Python remote-agent SDK bundled below `rust-server/python`. It also covers the contracts between those components. Space Game has its own game-mechanics and presentation tests and is outside this plan except where it serves as an end-to-end consumer fixture.

The target is confidence in behavioral invariants, not a nominal 100% line-coverage score. Every public behavior, security boundary, durable transition, asynchronous state machine, and error-recovery path must have an assertion. Coverage reports are useful for finding forgotten code, but a test that executes a line without proving its postconditions does not satisfy this plan.

As of 2026-08-15, the baseline is green:

- ordinary `cargo test` runs 116 Rust tests: 110 library tests, one redaction-binary test, four mock-client tests, and one Speechmatics contract test;
- the optional stress-TUI target contains three additional unit tests; and
- `npm test` runs 28 JavaScript tests in five files.

The server suite already exercises the main happy paths well. The largest gaps are deterministic failure injection, concurrent interleavings, lifecycle cleanup, security-boundary tables, remote-provider failures, and code that converts durable data for export. The client suite mostly tests pure helpers; it does not yet exercise the rendered startup state machine, `MicrophoneSource`, capture worklet, full audio sink lifecycle, browser teardown, or reconnect races. The Python SDK currently has no tests.

## Reliability properties

The suite should make the following properties explicit.

1. **Authorization is capability-based and scoped.** A participant credential or upgrade ticket grants only its participant, experiment, room, role, protocol, generation, and lifetime. Administrator mutations require an authenticated role, CSRF evidence, an allowed peer, and a valid same-origin request.
2. **Durability precedes visibility.** An accepted action, readiness declaration, consent change, conversation item, transcript, terminal transition, or configuration update becomes visible only after the corresponding required database transaction succeeds.
3. **Transitions are exactly once.** Concurrent actions, starts, completion, abandonment, expiry, reconnection, transcript delivery, configuration saves, and administrator setup cannot create duplicate transitions or contradictory terminal states.
4. **Private state does not cross a role or export boundary.** Participant messages contain only their observation. Credentials and configured secrets do not enter public configuration, logs, storage revisions, research exports, corpus exports, diagnostics, or package artifacts.
5. **Replacement is generation-safe.** A superseded game or audio connection cannot disconnect, publish for, mark ready, or mutate the state owned by its replacement.
6. **Queues are bounded and failure remains local.** Slow browsers, providers, or agents cannot produce unbounded memory growth. Overflow, lag, malformed input, provider closure, and cancellation do not corrupt another room or permanently wedge a registry.
7. **Timeouts are based on the intended clock.** Heartbeats affect transport liveness but not meaningful activity. Waiting, reconnect, idle, absolute-lifetime, ticket, credential, administrator-session, provider, and reconnect timers honor their exact boundaries.
8. **The client converges on the newest session generation.** Late promises, stale sockets, old timers, duplicate browser events, repeated button presses, and unmounts cannot revive or overwrite a newer session, microphone, or transport.
9. **Wire formats are shared contracts.** Rust, TypeScript, Python, protobuf, PCM framing, JSON field names, optional-versus-empty values, and package exports remain compatible.
10. **Operational tools are safe by default.** Catalogue merge and secret redaction are deterministic, collision-safe, transactional, idempotent where promised, and dry-run by default.

## Test architecture

### Layers

Use the smallest layer that can prove a property:

| Layer | Purpose | Normal runtime |
| --- | --- | --- |
| Pure unit and table tests | Parsers, validators, state projections, serialization, framing, bounded helpers | milliseconds |
| Property tests and fuzz targets | Large input spaces and parser invariants | short deterministic CI corpus; longer scheduled run |
| Component tests | One asynchronous object with controlled dependencies and clocks | milliseconds |
| Storage and protocol contract tests | Real SQLite, local HTTP/WebSocket/gRPC peers, transaction behavior | seconds |
| Runtime integration tests | Complete Axum router and two or more mock clients | seconds |
| Browser end-to-end tests | Real Chromium APIs, React lifecycle, package consumer behavior | seconds to minutes |
| Soak and adversarial load tests | Sustained scheduling, queue pressure, leaks, and deployment behavior | scheduled/manual |

### Determinism rules

- Use `tokio::time::pause`/`advance` and Vitest fake timers for logical time. Do not prove a race with arbitrary sleeps.
- Use barriers, oneshot channels, controlled futures, and a fault-injecting `ExperimentStore` to stop execution immediately before and after important writes.
- Assign every spawned task a deadline in the test. A hang must fail with a diagnostic rather than stall CI.
- Assert durable rows, in-memory state, and outbound messages together after a transition.
- Use unique temporary file-backed SQLite databases for lock, WAL, migration, restart, merge, and capacity tests. Use in-memory SQLite only where process/file semantics do not matter.
- Restore environment variables and browser globals after each test. Environment-dependent Rust tests run serially behind one shared guard.
- Do not assert probabilistic uniqueness. Assert format and namespace properties, and use injected deterministic id sources where collision handling must be tested.

### Proposed harness additions

- Rust: `proptest` for structured values, `cargo-fuzz` targets for untrusted parsers, and a test-only scripted/faulting `ExperimentStore`. Consider `loom` only for small extracted synchronization primitives; Tokio/WebSocket integration races should use explicit barriers.
- JavaScript: Vitest with `happy-dom` or `jsdom`, React Testing Library, `user-event`, and a controllable fake `WebSocket`, `MediaStream`, `AudioContext`, `AudioWorkletNode`, clock, and media-device registry.
- Browser: Playwright Chromium with fake microphone input and permissions. WebKit and Firefox run a smaller compatibility lane for URL, device, teardown, and WebSocket behavior.
- Python: `pytest`, `pytest-asyncio`, and generated protobuf modules in a temporary build tree.
- Coverage: `cargo llvm-cov --branch` and Vitest V8 branch coverage. Start with a ratchet equal to the measured baseline, then raise it per file as the catalog below lands. The final gate should be at least 90% statements/lines and 85% branches overall, with 100% branch coverage for authentication, lifecycle transition tables, secret redaction, PCM framing, and client session-generation guards. Explicitly reviewed unreachable/provider-platform branches may be excluded with a reason.

## Priority and acceptance

Tests are implemented in three gates.

- **P0 — integrity and race gate:** authorization scope, durable-before-visible failures, concurrent room/session transitions, connection generation, terminal-state rejection, transcript idempotency, startup/reconnect races, microphone/transport races, and malformed media/provider input. These block the reliability claim.
- **P1 — full behavioral gate:** remaining validators, exports/deletion, telemetry, UI states and accessibility, provider contracts, package surface, Python SDK, and all source-file traceability gaps.
- **P2 — sustained confidence gate:** fuzzing, multi-browser compatibility, resource/leak checks, long soak runs, deployment restart/disk pressure, and mutation testing.

Completion requires all P0 and P1 cases, no unexplained coverage regression, and a clean P2 scheduled run at least once on the release candidate. A flaky test is a defect: quarantine is allowed only with an owner, a reproduced failure record, and a short expiry.

## Rust server test catalogue

### R1. Configuration, descriptors, serialization, and utilities

1. Table-test every `ExperimentConfig::validate` boundary: empty/whitespace names; experiment-id alphabet, Unicode, and 128/129-character limits; zero/negative/exact timeout relations; every zero capacity; public URL schemes and missing authorities; origin paths, queries, fragments, credentials, ports, case, duplicates, and trailing slash; empty database URL; privacy version; jitter at one frame and 5000 ms; voice protocol constants; TTS provider/model/format; transcription provider/model/language/WebSocket URL; finite, zero, negative, NaN, and infinite floats; agent mode/object consistency; agent timeout and invalid-action limit; consent id/title/body limits and duplicates; and paired participant-information fields.
2. Table-test `activation_issues` for every independent and combined provider blocker, whitespace-only credentials, injected-provider overrides, and stable issue ordering.
3. Exercise YAML include cycles, self-include, diamond includes, optional missing includes, malformed include objects, scalar/array replacement, deep object merge, absolute/relative paths, a config outside a `config` directory, `sqlite:////` paths, `${VAR}` expansion with repeated/missing/empty values, literal dollar text, and unknown nested fields.
4. Verify `GameDescriptor` accepts exact boundary ids and rejects empty, invalid, overlong, and whitespace-only display names. Verify `PlayerRole`/`Seat` serialization and conversion contracts.
5. Snapshot the JSON protocol for every request, response, client-message, and server-message variant, including omitted, `null`, empty, and populated optional fields. Unknown fields must follow the documented strictness for each input type.
6. Property-test `AudioFrame::decode(encode(frame))`, identifier component normalization, CSV escaping, HTML escaping, Markdown escaping, origin canonicalization, and secret-field redaction idempotency.
7. Test `new_id`, room-code format, prefix handling, and forced room-code collision retry through an injectable id source. Test readable participant/dialogue shape and disjoint noun namespaces without making probabilistic uniqueness assertions.
8. Verify the public Rust API and rustdoc examples compile as an external crate, including the default `GameAdapter`, agent, and provider extension points.

### R2. Authentication and browser security boundaries

1. Participant credentials: unknown and malformed tokens, expiry at `now == expires_at`, revocation, multiple credentials for one participant, cleanup with one active sibling credential, generation checks, and concurrent issue/authenticate/revoke.
2. Upgrade tickets: exact expiry, wrong room, wrong purpose, replay, replacement issuance for the same tuple, independence across rooms/purposes/participants, concurrent double-consume with exactly one winner, and behavior when a failed wrong-scope consume destroys the ticket.
3. Administrator setup: username 0/1/128/129 boundaries, password 11/12/1024/1025, whitespace handling, invalid stored PHC, non-Argon2id PHC, and two simultaneous first-setup requests with one durable winner.
4. Administrator login/session: correct and incorrect username/password combinations, semaphore saturation, spawn-blocking failure propagation, idle/absolute exact boundaries, touch throttling, logout, cleanup, restart, malformed stored roles, and two concurrent touch/logout requests.
5. Route matrix for anonymous/operator/administrator roles across every administrator read and mutation. Mutations must also table-test absent/wrong CSRF, missing/foreign Origin, `Sec-Fetch-Site` values, Host/Origin ports and case, loopback aliases, HTTPS, malformed cookies, duplicate cookies, and allowed/disallowed IPv4/IPv6 CIDRs.
6. Participant route matrix for absent, malformed, revoked, cross-experiment, cross-room, cross-participant, wrong-role, and request-body identity injection. Test both HTTP and WebSocket upgrade paths.
7. CORS and WebSocket Origin table: configured exact origins, public origin, scheme downgrade, subdomain confusion, default/explicit ports, path/query suffixes, `null`, absent Origin according to policy, and duplicate Origin headers.
8. Security headers on success, redirects, 4xx, 5xx, static assets, SPA fallback, and WebSocket rejection. Assert nonce uniqueness and that the CSP nonce matches the rendered administrator script.
9. Adversarial stored content: HTML/script payloads in study names, notes, participant text, messages, actions, diagnostics, and provider errors must remain inert in dashboard HTML/JSON/CSV and must not produce spreadsheet formula execution in exported CSV.
10. Assert bearer credentials, tickets, administrator tokens, CSRF tokens, API keys, and secret values never occur in tracing output, durable revisions, normal configuration responses, privacy reports, or any export variant.

### R3. Storage, migrations, export, and operational tools

1. Create a backend-neutral conformance suite for every `ExperimentStore` method. Run it against SQLite and any future implementation. Cover missing ids, empty results, legal status changes, repeated idempotent calls, invalid foreign references, limits, ordering, and metadata round trips.
2. Concurrency: many parallel event appends produce gap-free unique event indexes; many session creates produce experiment-local monotonic ids; returning-participant upserts converge; stale configuration/game-setting revisions have one winner; session start has one winner; administrator setup has one winner; and terminal updates cannot overwrite each other.
3. Transaction fault injection at every statement of configuration+secret saves, game-setting+secret saves, accepted action+completion, expiry, abandonment, and participant deletion. After failure, assert no partial row, no advanced revision/index, no changed secret, no live state update, and a successful retry.
4. Database restart tests for WAL recovery, active-experiment deactivation, immutable configuration/session provenance, administrator sessions, migration idempotency, and readable identifiers.
5. Migration fixtures for every supported historical schema, not only selected migrations. Run each fixture directly to the current schema and through every intermediate version; compare schema, indexes, data, and a second no-op migration.
6. Export scoping: two experiments with overlapping session ids and mixed test/research data; missing session; empty experiment; all JSON shapes; stable ordering; no cross-scope participants, consents, or events; and repeated byte-stable output where the API promises stability.
7. Research/corpus export adversarial matrix: nested runtime ids, direct-identity keys at every depth, null/array/scalar oddities, malformed timestamps, negative relative times, duplicate participants, Unicode/newline/control characters, CSV commas/quotes/newlines/formula prefixes, testing-session graph removal, and stable pseudonyms.
8. Participant deletion: nonexistent and cross-experiment ids, multiple sessions/roles, content versus non-content events, ids embedded as exact strings and substrings, nested payload/state occurrences, repeated deletion, preview equality with applied counts, export after deletion, and rollback on failure. Prove unrelated participants and text are byte-for-byte unchanged.
9. File-backed capacity: main/WAL/SHM size accounting, missing sidecars, checkpoint changes, read-only/unwritable/full disk, reserve exact boundary, and health-check lock contention/timeout.
10. Catalogue merge: empty/source-only/target-only databases; dry-run leaves target byte-equivalent at the logical level; apply remaps every participant foreign key; administrator/game settings and secrets remain target-owned; source remains unchanged; collisions for experiment, room, dialogue, research, participant-session, and revision identifiers all abort before writes; same URL and same canonical path through different URLs reject; malformed/old sources migrate safely; row-count overflow rejects; detach/rollback occurs after injected failure; and a successful second merge is rejected rather than duplicated.
11. Redaction utility: nested maps/arrays, key case and separator variants, false-positive ordinary fields, non-string secret values, malformed JSON/YAML/arguments, check versus apply behavior, filesystem failure, and idempotent second application.
12. Storage queries and dashboard projections: limit 0/1/negative/large, deterministic sort ties, malformed historical JSON rows, event-type filters, participant joins with deleted/missing metadata, aggregate counts, completed timestamps, and experiment pin/status ordering.

### R4. Session admission, matchmaking, lifecycle, and cleanup

1. Table-test every `ExperimentLifecycle` parse, intake, purpose, and transition pair. Exercise the same table through HTTP and durable storage.
2. Admission boundaries at limit−1, limit, and limit+1 for unattached participants, waiting rooms, active rooms, transcription streams, and disk reserve. Test all limits under simultaneous requests using a barrier; successful reservations must never exceed the configured limit.
3. Matchmaking: simultaneous participant joins yield one room and distinct roles; the same participant retries idempotently; already-full rooms are skipped; agent rooms are skipped; testing and research purposes never mix; experiment/study/mode boundaries never mix; and a failed second-participant persistence does not leave an invisible in-memory member.
4. Human-agent room creation: one durable agent identity per room, factory selection and metadata, capacity accounting, agent-persistence failure rollback, and concurrent retries without duplicate agent participants or tasks.
5. Start race: simultaneous game-ready, audio-ready, reconnect, and agent-ready signals produce one durable `running` transition and one `roleAssigned` per current connection. A failed durable start leaves the room waiting and retryable.
6. Action race: simultaneous A/B actions serialize in one defined order, each validates against the latest state, event indexes match that order, completion occurs once, and all participant/agent observations correspond to the committed state.
7. Terminal race matrix: action-completion versus leave, completion versus expiry, leave versus expiry, two leaves, late transcript/chat/action versus each terminal state, and cleanup versus reconnect. Exactly one terminal status wins and later input is rejected consistently. Include `abandoned` and `expired`, not only `completed`.
8. Cleanup with a paused clock: exact waiting, reconnect, idle, maximum-lifetime, unattached-participant, terminal-retention, credential, and ticket boundaries. Heartbeats must not move meaningful activity; accepted actions/chat/audio at the documented cadence must. Malformed timestamps must have an explicit safe policy.
9. Cleanup candidate race: pause after candidate selection, mutate/reconnect the room, then continue cleanup. The observed-version check must preserve the current room. Pause after in-memory removal and inject durable expiry failure to define and test recovery.
10. Runtime-cache invalidation: concurrent first requests construct one experiment runtime; inactive configuration saves invalidate only the selected runtime; active saves reject; stale weak references disappear; cross-experiment rooms, buses, tickets, credentials, agents, and telemetry remain isolated.
11. Root and static routing: canonical trailing slash, percent-encoded experiment ids, unknown experiments, API prefixes, path traversal and symlinks, dot files, missing index/dist directory, cache/content types, range/HEAD/OPTIONS behavior, and unscoped root/admin redirects.
12. Deactivation and process restart: new participant/room intake closes while established sessions continue, all open experiments reset on restart, terminal states remain, invalid active configuration fails closed, and historical storage-only admin routes remain readable without constructing a runtime.

### R5. Game WebSocket, conversation, and broadcast behavior

1. Full client-message table: heartbeat, ready, consent update, leave, chat, action, unknown type, invalid JSON, binary/ping/pong/close, missing fields, unknown fields, 4 KiB action boundary, 4,000-character chat boundary with multibyte Unicode, 64 KiB transport boundary, and fragmented WebSocket messages.
2. Heartbeat and transport token buckets at exact capacity/refill boundaries with paused time. Rejection aggregation must persist the first item, suppress within the window, flush on current-owner disconnect, retain bounded hashes/lengths, and never store the raw oversized input.
3. Connection replacement interleavings: old socket closes before/after new registration, old receive loop handles an already-read message, old timeout fires, and old unregister runs after new disconnect. Only the current generation may set `connected=false` or persist the final disconnect.
4. Broadcast pressure: exceed the 256-message room bus while a receiver is paused. Define and assert recovery on `Lagged`—resynchronize or close/reconnect explicitly—rather than silently terminating outbound delivery while leaving input open.
5. Targeting/privacy: role assignment and state changes reach only intended/current sockets; role-specific observations never contain the other role's private fields; presence contains no participant-session ids or credentials; cross-room and cross-experiment fan-out is impossible.
6. Conversation origins: participants cannot forge agent/system/voice origin or another sender through metadata; empty/whitespace text policy; metadata byte boundary and non-object values; storage switches; message ordering; duplicate source ids; room completion/abandonment/expiry; and agent notification modality.
7. Ready/consent durability failures: pause after tentative memory mutation and fail storage, assert rollback and retry. Duplicate ready and unchanged consent remain no-op writes under concurrency.
8. Required-store failure on action, chat, transcript, readiness, join, and terminal events must yield an error without a success broadcast or live-state change. Best-effort diagnostics must fail locally without changing semantic success.
9. Reconnection after missed events always receives a complete current `roleAssigned` snapshot, current actions and conversation, not merely subsequent deltas. Stale ticket and role changes reject.
10. Session idle behavior distinguishes heartbeat, rejected input, partial transcription, invalid audio, accepted chat/action/final transcript, ready, and connect/disconnect according to the documented meaningful-activity policy.

### R6. Audio relay, transcription, and TTS

1. PCM framing boundaries and properties: all integer extrema, endianness, exact 973 bytes, wrong versions, every short/long length, encode with an invalid PCM length, and fuzz decode with arbitrary bytes without panic.
2. Registry matrix: A→B, B→A, unknown roles, missing rooms/peers, control targeting, agent fan-out, room isolation, reconnect generations, disconnect idempotency, and automatic empty-room removal.
3. Queue saturation: fill each bounded peer queue, prove sends remain nonblocking, identify which frames are dropped, prove other peers/rooms remain live, and expose accurate drop telemetry where promised.
4. Audio WebSocket input: sequence duplicates, gaps, wrap, regression, timestamp equality/regression/overflow, far-ahead wall time at the exact allowance, invalid text/binary/control frames, close/error, 90-second silence, and long valid streams. Dropped-frame diagnostics obey the privacy switch and aggregate exactly once.
5. Audio replacement/provider race: an old transcription task emits Ready, Failed, Partial, or Final after a newer audio generation owns the role. Old generations must not mark the participant ready, commit transcripts, change current status, or disconnect the new relay.
6. ASR startup success/failure/10-second timeout, input backpressure, output channel closure, finish backpressure/2-second timeout, event-task shutdown/5-second timeout, provider panic/cancellation, and partner-audio continuity in every failure.
7. Final transcript validation and idempotency: repeated result ids, empty result-id fallback collisions, reordered ids, concurrent duplicates, Unicode/empty/huge text, negative/reversed times, storage failure then retry, privacy-disabled storage, and terminal-room rejection. Idempotency memory must remain bounded and generation/session scoped.
8. Speechmatics local contract variants: auth rejection, malformed JSON, non-text frames, Error before/after RecognitionStarted, partial accumulation, multiple final fragments, empty alternatives, missing timing, non-finite/negative seconds, provider close before/after final, EndOfStream ordering, input closure, language subtags, default versus named models, and optional conversation settings.
9. TTS local contract variants: URL path/query encoding, auth/config messages, binary and malformed JSON frames, invalid base64, empty audio, multiple chunks, final without audio, close without final, provider error, sample-rate mapping ambiguity, and cancellation/timeouts.
10. `RoomAgentAudioPublisher`: format/channel rejection, empty and final-only chunks, concatenation across odd chunk sizes, 960-byte boundaries, final-frame padding, sequence/timestamps, prebuffer sizes 0/1/N, paused-time pacing, queue pressure, room isolation, summary accuracy, and cancellation midway.
11. Agent TTS integration: synthesis success with and without a publisher, empty provider result, publish failure, provider timeout, concurrent agent messages, completion/abandonment during synthesis, diagnostic ordering/content minimization, and telemetry gauge release after every error or cancellation.
12. Scheduled real-browser audio tests: fake 24 kHz tone capture through the worklet and relay to a second browser, mute/unmute, device switch, reconnect, jitter/packet delay, underrun recovery, and no feedback into transcription for server-generated TTS.

### R7. Agent runtime and remote gRPC

1. Default trait behavior and `AgentResponse::is_empty` for no/message/action/both variants. Verify factory identity defaults and metadata serialization.
2. Agent creation failure, panic, initial observation timeout/error, maybe-act `None`, invalid/empty response, parse/rule failure, observation timeout/error, shutdown error/timeout, room terminal state, and channel closure. Every path must remove `started_agents`/inboxes and leave a restart policy that is explicit and testable.
3. Race agent start from room-ready/reconnect/audio-ready signals; create one instance. Race participant action/message delivery with agent startup and shutdown; define whether pre-inbox observations are replayed or intentionally absent.
4. Fill the 64-item agent inbox with a stalled agent. Participant transitions must remain bounded; registry read locks must not be held across a blocking send, and cleanup/shutdown must still acquire write access.
5. Agent response containing action and message: assert the defined order, resulting observations, completion behavior, message persistence, and whether a message is permitted after its action completes the room.
6. Remote endpoint validation table: loopback names and IPv4/IPv6 syntax, DNS aliases resolving to loopback, HTTP non-loopback, HTTPS with/without token, allowlist trimming/case/ports/subdomain confusion, malformed URLs, user info, fragments, and environment cleanup.
7. gRPC lifecycle: create once, auth metadata present but never logged, all observation kinds, `available_actions` omitted versus empty versus populated, required/optional response, timeout/status/malformed payload, empty agent id, shutdown, reconnect/retry policy, and service closure midway.
8. Protobuf conversion: nested objects/lists/null/bools/strings, empty keys, large and fractional numbers, values beyond JavaScript/protobuf-Struct exact integer range, NaN/infinity from a malicious peer, top-level non-object actions, missing kinds, and invalid typed actions.
9. Protocol-version mismatch and agent-name/version identity must fail or round-trip according to the published contract. Keep the Rust proto and Python-bundled proto byte-identical in a drift test.

### R8. Telemetry and stress tooling

1. Every telemetry counter and gauge: increment, concurrent increment, maximum in-flight, success/4xx/5xx classes, latency bucket boundaries, TTS begin/finish balance, and RAII drop without explicit finish.
2. Connection liveness with an injectable clock: initial snapshot, payload, heartbeat, both orderings, live/delayed/stale exact thresholds, saturating time subtraction, and timestamp monotonicity.
3. Load sampling: waiting/active/completed/unattached/transcription/agent counts, strictest storage reserve, missing/error capacity, pending rejection sum, multiple runtimes, stale weak refs, empty registry fallback, deterministic experiment/session ordering, and one-hour ring-buffer truncation.
4. Lifecycle deadlines and session health table across all room statuses, agents versus humans, no participants, malformed timestamps, multiple candidate deadlines, reconnects, live/delayed/stale/disconnected transports, and terminal cleanup.
5. Stress-TUI parser and model: every CLI boundary, zero/overflow sizes, mode defaults, deterministic impairment schedule, counter overflow, latency quantiles with empty/small samples, terminal cancellation, failed invariant exit code, and terminal restoration after panic/error.
6. Scheduled stress acceptance: realistic, impaired, and saturation modes with fixed seeds and machine-normalized thresholds. Assert payload integrity, cross-room isolation, expected versus unexpected drops, bounded resident memory, no task growth after teardown, and reproducible report artifacts.

## JavaScript client test catalogue

### J1. Protocol and package surface

1. `apiBase` for canonical experiment paths, nested participant routes, trailing slash, percent encoding, missing experiment prefix, query/hash, and origins with ports.
2. `socketUrl` for HTTP/HTTPS, existing queries and token replacement, fragments, absolute/relative/protocol-relative paths, encoded tokens, IPv6, and invalid URLs. Assert caller input is not mutated.
3. Every `ExperimentApiClient` method: exact URL, method, headers, JSON body, bearer behavior, response shape, and no credential in URLs. Calls before participant creation throw synchronously where intended.
4. Participant recreation and overlapping create requests: define which credential wins. A failed later creation must not erase the last valid credential; a late earlier response must not overwrite a newer session.
5. `checkedJson`: JSON success, falsy payloads, 204/empty success, invalid success JSON, text/JSON/empty error bodies, body-read failure, network rejection, abort, and status preservation in the thrown error contract.
6. `sendAction`/chat/leave across null, CONNECTING, OPEN, CLOSING, and CLOSED sockets; JSON serialization failure; and a socket whose `send` throws. The API must not crash a rendered client during an ordinary disconnect.
7. Voice diagnostics: keepalive, auth, metadata, fetch rejection, serialization failure, and call after participant/session replacement. Confirm diagnostics never surface an unhandled rejection.
8. Build, pack, install the tarball into a temporary strict TypeScript consumer, then import root and `/react` entrypoints in ESM. Assert declarations, declaration maps, worklet assets/import URLs, peer dependency behavior, file allowlist, and absence of source tests/secrets/local paths.
9. Compile-only type tests for generic state/observation/action/event/summary inference, omitted optional properties, custom roles/origins, and both entrypoints.

### J2. Pure helpers and React widgets

1. Required-consent matrices: null config, empty list, required/optional mixes, false/missing/extra decisions, duplicate ids, and prototype-like ids such as `__proto__`.
2. Intake statuses including null/undefined and future unknown runtime strings at the untyped boundary.
3. Presence normalization for null, arrays, primitives, inherited properties, unknown roles, string truthiness, missing/invalid booleans, and explicit false values. Confirm it does not leak unexpected fields.
4. Transcription progress for every known status, unknown/empty values, ready overriding status, exact step classes, and stable value range.
5. Labels and device filtering for all voice states, whitespace/Unicode device names, USB suffix variants, duplicate/empty device ids, synthetic aliases, and absent institution/title.
6. Render `VoiceJoinButton`, `DeviceSelect`, chips, meters, transcription progress, readiness, and preparation controls. Assert accessible names/roles, disabled rules, callback counts/values, fallback options, error class, progress clamping, and keyboard operation.
7. `useVoiceController` subscribes once, renders the immediate snapshot, responds to updates, resubscribes on controller change, unsubscribes on unmount, and ignores post-unmount notifications.
8. Accessibility checks for every startup state using `axe`, plus keyboard-only consent, microphone selection, enter, and leave flows.

### J3. `ParlandoStartupGate` state machine

1. Rendered happy paths for voice-disabled human-human and voice-enabled human-agent sessions from config load through active game and completion.
2. Config load: pending, success, rejection, rejection after unmount, and a replaced `apiClient` whose old promise resolves late. Closed-intake polling starts/stops at the right states, handles errors, and ignores stale results.
3. Consent: required gating, optional values, participant-information link, create→submit-consent→room ordering, failure at each step, double-click prevention/idempotency, and config changing while the flow is pending.
4. Microphone preparation: permission success/failure, device list before/after permission, devicechange, enumerate rejection, selected-device disappearance, rapid device switches with reversed promise resolution, and unmount during preparation.
5. Game connection: ticket success/failure, WebSocket constructor failure, open/error/close orderings, close before open, role assignment, all server-message variants, binary/malformed/unknown JSON messages, and duplicate messages.
6. Stale socket generation: after reconnect or room replacement, old open/message/error/close callbacks must not alter the current session, error, connection flag, completion, presence, conversation, or reconnect schedule.
7. Reconnect backoff at 1/2/5/10 seconds, reset after open, repeated close coalescing, ticket failures, five-minute exact expiry, completion/leave/abandonment/unmount cancellation, and no overlapping reconnect attempt.
8. Heartbeat starts only for the current session socket, sends only while OPEN, moves to a replacement socket, stops on session removal/unmount, and creates no duplicate interval after rerender/reconnect.
9. Server message semantics: omitted observation preserves the previous observation where documented; omitted state, actions, events, and conversation follow their respective rules; event ordering/cap 20; conversation dedupe/cap 50; completion summary nullability; and terminal messages block all later input.
10. `abandoned` disconnects voice, disables reconnect, closes only the owning socket, clears the session, and shows the server/fallback message. Test `expired` if/when represented explicitly; until then, prove the server maps it to a handled terminal message.
11. Leave/pagehide/beforeunload/unmount may arrive in every order and more than once. Exactly one intentional leave is sent while OPEN, sockets in OPEN/CONNECTING close, voice resets, timers stop, and no state update occurs after unmount.
12. Voice auto-connect: only after game connected + preflight ready, no duplicate while connecting/connected, retry progression after rejection, reset after success, current session captured, and cancellation on leave/device reset/unmount.
13. User actions/chat during disconnect, reconnect, completion, leave, and stale render callbacks never throw or reach a closed/stale socket. Decide whether disconnected-but-not-completed input is queued, dropped, or rejected and assert that contract.
14. Error precedence/recovery: network, config poll, microphone, ticket, socket, reconnect, voice, and server errors. A later stale success must not clear a current error; a successful current operation clears only the error it owns.

### J4. Microphone and audio-session controller

1. `MicrophoneSource.input` before prepare; prepare with default and exact device constraints; no returned audio track; empty labels; track cloning; same-device reuse; device replacement; secure-context and missing-API failures.
2. Permission rejection classes preserve the underlying error while exposing the stable participant message. Partial streams/tracks acquired before failure are stopped.
3. Concurrent prepare A/B with every completion order. Only the newest request may own state; stale success must stop its own stream and stale failure must not reset a newer success.
4. Stop/reset idempotency, all tracks stopped once, probe teardown, pending prepare completion after stop/reset, and listener subscribe/unsubscribe/reentrant unsubscribe behavior.
5. Probe with no AudioContext, resume rejection, analyser values at silence/full range, RMS clamp, repeated animation frames, stop during tick, source disconnect/context close failure, and no callback after cleanup.
6. Controller snapshots are immutable-by-replacement, subscriptions are immediate and removable, partial status merges preserve fields, and listener mutation during emission is safe.
7. Prepare status semantics while connected/disconnected and on failure. Disconnect with/without reset, sink disconnect rejection, microphone stop/reset ordering, and state after partial teardown failure.
8. Toggle connected mute/unmute success/failure and diagnostic ordering. Failed `setInputEnabled` must not claim the requested microphone state.
9. Toggle disconnected connect success/failure, missing microphone, sink callback races, and preservation of prepared input. `connecting` must always return to false after every outcome.
10. Concurrent/reentrant toggle, prepare, disconnect, and status callbacks. At most one transport connect and one mute transition may own the controller at a time; stale sink callbacks must not resurrect a disconnected generation.

### J5. Audio sink and worklets

1. `encodeFrame` property tests for u32/u64 boundaries, negative/fractional/NaN/infinite JS inputs, overflow behavior, immutability, endianness, exact PCM copy, and invalid buffer lengths.
2. `waitForSocketOpen` for open/error/close and all event orderings, listener cleanup, already-open/already-closed sockets, and abort/timeout policy.
3. Full sink connect with fake Web Audio: prior disconnect, disabled/incomplete plan, worklet load failure, resume failure, stream/source/node construction failure, socket constructor/open failure, and success. Every partial failure must close/stop/disconnect all acquired resources.
4. Verify worklet module URLs, jitter-sample rounding/bounds, graph wiring, cloned stream ownership, socket URL/token, binary type, status sequence, and diagnostics.
5. Capture callback sends only while enabled and OPEN, increments/wraps sequence as defined, uses monotonic relative timestamps, handles `encodeFrame`/`send` failure, and stops after replacement/disconnect.
6. Incoming text: every known transcription status, missing fields, unknown type, malformed JSON, and non-string messages. Malformed provider control must not throw out of the browser event loop.
7. Incoming binary: exact frame, short/long frame, non-ArrayBuffer Blob/view, transfer to playback, remote-audio status, and no processing by a stale socket after reconnect.
8. Close/error races: current close updates status once; stale close from a replaced socket does nothing; disconnect followed by close cannot emit a misleading participant-visible state; unexpected close releases the audio graph or has an explicit reconnect owner.
9. `setInputEnabled` before/during/after connect and across reconnect. Disconnect is idempotent and survives each node/track/context throwing during teardown.
10. `PcmPlaybackBuffer`: empty output, empty input, min/max PCM16 normalization, multiple appends, source/output ratios below/equal/above one, fractional cursor across renders, one-sample interpolation, exact exhaustion, repeated underrun, trim 0/negative/fractional/large, and bounded long-stream memory.
11. Playback processor with a fake worklet global: jitter configuration min/rounding, initial threshold, underrun resume threshold, output absence/multiple channels, sample-rate variants, stale-buffer trim, underrun count/diagnostic, and indefinite `true` return.
12. Capture processor with a fake worklet global: 24/44.1/48/96 kHz, empty/missing input, quantum boundaries, phase continuity, exact 480-sample frames, interpolation, clamp, positive/negative PCM conversion asymmetry, multiple frames, transferable buffers, and long-run drift bound.
13. Cross-worklet deterministic signal test: resample a known tone/noise through capture framing and playback at each common device rate; bound amplitude, duration, discontinuity, and accumulated clock drift.

## Python remote-agent SDK catalogue

1. `AgentResponse` constructors, frozen behavior, empty-response rejection, and nested action conversion.
2. Struct/dict conversion for nested values, null, booleans, Unicode, empty structures, large/fractional numbers, and invalid protobuf values.
3. Available-actions omitted versus empty versus populated, and every utterance-kind mapping including unknown enum values.
4. Service method matrix with a fake context: authentication absent/correct/wrong/malformed, unknown agent ids, sync/async factories, class factory, factory exception/cancellation, capacity exact boundary, all observation callbacks, optional/required responses, shutdown twice, and close error.
5. Concurrent `CreateAgent` calls at `max_agents` must not exceed capacity. Concurrent RPC and Shutdown on one agent must have an explicit outcome and never access a removed object unsafely.
6. Validate `max_agents` and port/host inputs, TLS key/certificate pairing, loopback aliases, non-loopback TLS plus mTLS/token policy, environment-token fallback, secure/insecure port failure, server start failure, termination cancellation, and graceful stop.
7. Generate protobufs into a temporary package, import them, build a wheel/sdist, install into a clean environment, and run a minimal client/server exchange. Assert `py.typed` and `.proto` package data.
8. Regenerate the Python bindings from the shared `proto/parlando_agent_v3.proto` in CI and fail on an unexpected diff.

## Cross-component and browser acceptance tests

1. Generate canonical protocol fixture cases in Rust and consume them in TypeScript and Python: public config, room response, every server/client message, conversations, completion, nested actions, and optional/empty semantics.
2. Generate canonical PCM frames in Rust and JavaScript and compare bytes in both directions, including integer boundary fields.
3. Start a real Rust router and drive it using the built npm tarball in Chromium: closed intake, consent, human-human pairing, actions/chat, completion, leave, and reconnect.
4. Repeat for human-agent and remote Python gRPC agent, including available actions, observation order, agent message/action, shutdown, and TTS diagnostics with local fake providers.
5. Two simultaneous experiments with overlapping ids: browser URL derivation, credentials, tickets, rooms, messages, audio, telemetry, and exports remain isolated.
6. Restart the Rust process with a live browser: intake closes fail-safe, the client displays the resulting state, historical durable data remains, and no credential/session is silently treated as resumed.
7. Browser audio acceptance with fake media: two clients exchange known PCM, transcription receives only browser audio, agent TTS reaches the human, mute blocks outbound capture, replacement revokes the old generation, and device switch does not leak tracks.
8. Adversarial network proxy: delay, duplicate, reorder, fragment, corrupt, pause, and close game/audio frames. Assert bounded recovery, no cross-room data, no unhandled browser rejection, and correct durable audit rows.
9. Package compatibility lane against the oldest supported Rust compiler/dependency lock, Node/TypeScript versions declared by policy, Python 3.10+, and current Chromium/Firefox/WebKit. Add an explicit support matrix before enforcing this lane.

## Fuzzing, model checking, and mutation tests

- Continuous short fuzz targets: audio-frame decode, client-message JSON, configuration YAML/include normalization, remote-agent JSON↔protobuf conversion, export sanitization/CSV escaping, diagnostic metadata minimization, and WebSocket control-message parsing.
- Property-based state machine: generate participant creation, consent, room entry, connect/reconnect, ready/audio-ready, action/chat/transcript, leave, timeout, and restart commands. Compare the runtime against a small reference model for role uniqueness, lifecycle legality, exactly-one terminal status, event ordering, and capacity.
- JavaScript state-model tests: generate socket/timer/promise/device/browser lifecycle events and assert that only the newest generation owns rendered state and resources.
- Run mutation testing selectively on `auth.rs`, `app/lifecycle.rs`, `submit_action`, ticket consumption, configuration validation, export sanitization, PCM framing, startup generation guards, and microphone/audio controller transitions. Surviving mutations become missing test cases; mutation score is diagnostic rather than a release vanity metric.
- Scheduled fuzz corpora and failing seeds are committed after minimization. No test relies on an unrecorded random seed.

## Source traceability matrix

| Source | Primary planned families |
| --- | --- |
| `rust-server/src/app.rs` | R2, R4, R5, R6, R7, R8 |
| `rust-server/src/app/lifecycle.rs` | R4 |
| `rust-server/src/app/routing.rs` | R2, R4 |
| `rust-server/src/app/telemetry.rs` | R8 |
| `rust-server/src/storage.rs` and migrations | R3 |
| `rust-server/src/auth.rs` | R2 |
| `rust-server/src/config.rs` | R1 |
| `rust-server/src/game.rs`, `agents.rs`, `protocol.rs` | R1, R7 |
| `rust-server/src/audio.rs`, `audio_publisher.rs` | R6 |
| `rust-server/src/transcription.rs`, `tts.rs` | R6 |
| `rust-server/src/remote_agent.rs` and proto | R7, cross-component |
| `rust-server/src/identity.rs`, `readable_id.rs`, `lib.rs` | R1 |
| Rust command-line binaries | R3, R8 |
| Python agent SDK | Python catalogue, R7, cross-component |
| `js-client/src/protocol.ts`, `helpers.ts`, `index.ts` | J1, J2 |
| `js-client/src/startup.tsx` | J2, J3 |
| `js-client/src/react.tsx`, `voiceComponents.tsx` | J2 |
| `js-client/src/audio/microphoneSource.ts` | J4 |
| `js-client/src/audio/audioSessionController.ts` | J4 |
| `js-client/src/audio/parlandoAudioSink.ts` | J5 |
| `js-client/src/audio/captureWorklet.ts`, `playbackWorklet.ts` | J5 |
| package manifests/build output | J1, Python catalogue, cross-component |

## Likely defect probes

The following tests deserve first implementation because the current structure makes them plausible defect finders rather than generic coverage work:

1. reversed completion of two `MicrophoneSource.prepare` calls;
2. concurrent controller `toggle` calls and stale sink callbacks after disconnect;
3. stale game-socket callbacks and reconnect timers after a replacement session;
4. malformed JSON text on the audio socket;
5. an old transcription task emitting Ready or Final after audio replacement;
6. agent-factory creation failure and agent-inbox saturation cleanup;
7. room-broadcast lag exceeding 256 messages;
8. chat/transcript arrival after `abandoned` or `expired` rather than only after `completed`;
9. concurrent action/completion/leave/expiry with a required database failure inserted at each boundary;
10. participant deletion where one participant-session id is a substring of unrelated stored text;
11. protobuf `Struct` conversion for integers outside the exactly representable double range.

## Implementation sequence

1. Add deterministic test clocks, WebSocket/media fakes, the faulting store, fixture builders, and source-contract fixtures.
2. Implement the likely defect probes and all P0 authorization/durability/terminal-state cases. Fix product defects as separate reviewed changes when a test demonstrates one.
3. Complete client component/worklet and server storage/configuration/security matrices.
4. Add Python and package-consumer tests, then shared Rust/TypeScript/Python contract fixtures.
5. Add browser end-to-end lanes, fuzz/property targets, coverage ratchets, and scheduled soak jobs.
6. Review coverage and mutation survivors by source file; add semantic assertions for any unexplained gap and document deliberate exclusions.

This order builds reusable control points first and keeps failures attributable. It also avoids creating a large end-to-end suite that can detect a regression without identifying which invariant failed.
