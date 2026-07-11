# Test Todos

## Context

Steps 5-11 are complete at the Rust server-library level, but the server still needs additional regression and production-confidence tests before it should be treated as a fully proven drop-in replacement for the existing Python server. This file tracks tests we intentionally have not added yet.

## Client Drop-In Tests

- Run the existing TypeScript client against the Rust server with two browser participants.
- Verify browser WebSocket gameplay, reconnect behavior, typed chat, transcript POST, and completion.
- Verify human-vs-agent gameplay from the browser, including visible agent messages and accepted agent actions.
- Verify the browser accepts the Rust `audio-session` response in both LiveKit-only and Speechmatics-enabled modes.
- Verify Speechmatics transcription from the browser microphone path appears in the expected UI flow and database event stream.

## WebSocket Adversarial Tests

- Send malformed JSON and verify the connection receives a clean error without panicking the server.
- Send an unknown message type and verify the error shape remains client-compatible.
- Attempt to connect with a participant session ID that does not belong to the room.
- Send duplicate `ready` messages and verify state remains stable.
- Reconnect after completion and verify the participant receives a coherent completed state.

## Concurrency And Race Tests

- Submit near-simultaneous actions from roles `A` and `B` and verify only valid state transitions are accepted.
- Attempt duplicate joins for the same participant session under concurrent requests.
- Attempt concurrent room creation or matchmaking joins and verify session IDs remain unique within each experiment.
- Verify event indices remain gap-free and ordered under concurrent activity.

## Export And Evaluation Tests

- Reconstruct a session from `session_events` alone and verify it matches the final stored completion summary.
- Verify experiment export remains easy to query by `experiment_id`, `session_id`, event type, actor participant, and role.
- Verify transcript, conversation, voice diagnostic, agent, TTS, and game action events have enough payload detail for evaluation without category tables.

## Agent Runtime Tests

- Add an explicit agent act-timeout test distinct from invalid-action stopping.
- Verify an agent that returns only messages, only actions, both, or neither is handled correctly.
- Verify the agent receives exactly the same observation and optional available-action affordance that a human role can see through the public protocol.

## Remote Agent Tests

- Implement and test the planned gRPC remote-agent bridge.
- Add a small Python remote-agent test server using the intended clean Python API.
- Verify remote-agent latency and throughput are acceptable for reinforcement-learning-style usage.
- Verify remote-agent failures, disconnects, invalid actions, and timeouts are persisted as session events.

## Live Audio And Deployment Tests

- Prove browser audio in a real LiveKit room with Rust-minted tokens.
- Decide and test the LiveKit agent-audio publishing path or documented sidecar fallback.
- Add packaging and deployment smoke tests for static serving and production startup.
