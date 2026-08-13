# Audio Testing

Parlando separates deterministic correctness tests from a standalone interactive stress program. No automated test connects to Speechmatics, ElevenLabs, or another paid service, and no test requires provider credentials.

## Fast Tests

Run all Rust unit, protocol-contract, and integration tests with:

```sh
cd rust-server
cargo test
```

This ordinary suite includes frame validation, token authorization and replay rejection, simultaneous-room isolation, connection replacement, relay and transcription fan-out, transcript idempotency, agent delivery, TTS publication, and a credential-free Speechmatics adapter contract test. That contract test starts a local fake provider WebSocket and exercises the production adapter's authorization header, recognition configuration, PCM transfer, partial/final events, utterance timing, and end-of-stream sequence.

TypeScript tests use their own toolchain:

```sh
cd js-client
npm install
npm test
```

They cover binary framing, playback resampling and underruns, microphone/transport lifecycle, startup behavior, and WebSocket establishment failures.

## Interactive Stress Dashboard

The standalone `audio-stress-tui` binary repeatedly relays canonical 973-byte PCM frames through many simultaneous in-memory rooms. Every frame contains its room, source, and round identity, and the receiver verifies the complete encoded frame. A mismatch, timeout, unexpected control message, or unplanned queue drop ends the run unsuccessfully. By default it runs the `realistic` mode for ten minutes with 200 rooms:

```sh
cd rust-server
cargo run --release --features stress-tui --bin audio-stress-tui
```

`realistic` sends one frame from each participant every 20 ms in full duplex, matching the browser transport's production cadence. It also selects 10% of rooms every five seconds for a simulated 200 ms agent-TTS burst. All workload settings are command-line arguments; the stress runner does not read configuration from environment variables.

The dashboard shows only measurements produced by the workload:

- overall elapsed-time progress;
- active rooms and streams, participant frames, and simulated agent-TTS frames;
- current, average, and peak verified frames per second;
- a rolling 60-second verified-frame throughput sparkline;
- rolling p50, p95, p99, and maximum in-memory relay latency;
- one latency-colored activity tile per room, or queue-pressure tiles in impaired mode;
- separate payload/cross-room, timeout, unexpected-message, queue-drop, and scheduler-deadline counters;
- real milestones such as new throughput peaks and each million verified relays.

Press `q` or `Esc` to stop cleanly. The terminal is restored before the program exits. A completed or user-stopped healthy run exits successfully; any failed invariant exits nonzero.

Use CLI arguments to select the workload and its size. For example, this runs the production-shaped workload for one minute with 500 rooms and more frequent TTS activity:

```sh
cargo run --release --features stress-tui --bin audio-stress-tui -- \
  --mode realistic \
  --rooms 500 \
  --seconds 60 \
  --tts-room-percent 20 \
  --tts-interval-seconds 2
```

Three modes serve different purposes:

- `--mode realistic` is the default acceptance soak. It verifies full-duplex 20 ms participant traffic and periodic agent fan-out at a stable offered load. Its expected base rate is `rooms × 2 × 50` participant frames per second; agent frames are additional.
- `--mode saturation` removes pacing and measures the maximum verified throughput of the in-memory registry. Use it for comparative performance work, not as a model of a real call.
- `--mode impaired` retains realistic cadence while deterministically adding ±2 ms scheduling jitter and pausing 5% of room consumers for 1.6 seconds every 15 seconds. Bounded-queue drops and deadline misses are expected observations in this mode; payload corruption, cross-room delivery, timeouts, and unexpected control messages still fail it.

Run the other modes explicitly:

```sh
cargo run --release --features stress-tui --bin audio-stress-tui -- \
  --mode saturation --rooms 200 --seconds 60

cargo run --release --features stress-tui --bin audio-stress-tui -- \
  --mode impaired --rooms 200 --seconds 600
```

Run `cargo run --release --features stress-tui --bin audio-stress-tui -- --help` for the complete CLI. Ratatui and the CLI parser are optional `stress-tui` dependencies, so ordinary server builds and `cargo test` do not compile or ship them. Use `--release` for meaningful saturation numbers; debug-build rates primarily measure Rust debug overhead.

The latency panel measures only in-memory send-to-verification time. The program tests the process-local `AudioRoomRegistry`, canonical frame integrity, bounded-queue behavior, queue liveness, and strict room isolation under sustained activity. Its TTS traffic is PCM fan-out rather than speech synthesis. It does not open browser or network WebSockets and does not simulate browser scheduling, TCP/WebSocket behavior, reverse proxies, STT, or actual TTS latency. Those boundaries remain covered by deterministic integration tests and deliberate deployment checks.

## Deliberately Excluded Live-provider Tests

The repository does not contain a real Speechmatics smoke test and should not receive Speechmatics credentials through GitHub or a shared test environment. Provider behavior is represented by local deterministic protocol peers. A developer may manually test a private deployment against a real provider, but that is an operational check rather than part of the repository's automated acceptance suite.

## Remaining Production Checks

Before a high-volume study, run the ten-minute dashboard in both realistic and impaired modes, manually exercise two real browsers under representative network conditions, and perform a longer deployment soak through the actual reverse proxy. Multi-instance deployments must additionally verify sticky room routing because active audio rooms are process-local.
