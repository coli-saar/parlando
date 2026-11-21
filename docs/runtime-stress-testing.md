# Runtime Stress Testing

`runtime-stress` drives a real loopback Parlando server through public HTTP,
game-WebSocket, and audio-WebSocket endpoints. It uses a temporary file-backed
SQLite database and the same Ratatui dashboard as `audio-stress-tui`.

The executable supports human-human and human-agent sessions. Both paths use
participant creation, consent, room admission, one-use game and audio tickets,
readiness, typed messages, accepted actions, and canonical PCM frames every 20
ms. Human-human runs verify every relayed audio payload byte-for-byte.
Human-agent runs create a session-local agent through the normal factory and
callback lifecycle, feed human audio to deterministic local ASR, synthesize
agent messages with deterministic local TTS, and receive the agent speech over
the participant's real audio WebSocket. The local providers deliberately add
small asynchronous delays but require no credentials or paid calls.

Every run has four phases in fixed 1:4:3:2 proportions:

| Phase | Acceptance duration | Behavior |
| --- | ---: | --- |
| Ramp | 1 minute | Pace admissions up to target concurrency |
| Steady | 4 minutes | Sustain game, message, action, ASR, TTS, and audio traffic |
| Churn | 3 minutes | Replace both WebSockets for a deterministic quarter of sessions |
| Drain | 2 minutes | Stop traffic, send terminal actions, close sockets, and collect evidence |

The same proportions are retained for shortened runs, so a 60-second smoke run
uses 6/24/18/12 seconds. The minimum override is 10 seconds.

Run a short non-interactive smoke test:

```sh
cd rust-server-tests
cargo run --features stress-tui --bin runtime-stress -- \
  --pairing human-agent --sessions 2 --seconds 20 --headless \
  --output /tmp/parlando-runtime-stress
```

Run the interactive dashboard with more sessions:

```sh
cd rust-server-tests
cargo run --release --features stress-tui --bin runtime-stress -- \
  --pairing human-agent --sessions 100 --seconds 600
```

Use `q` or `Esc` to request a clean stop. `Tab` cycles dashboard time-series
views. Use `--keep` to preserve the temporary directory for SQLite inspection;
failed runs preserve it automatically. Use `--output PATH` to select the stable
artifact root. Each run creates a timestamped directory under that root and
writes a JSON report containing the resolved workload, correctness counters,
reconnects, ASR/TTS counts, steady-phase action latency, SQLite row
counts, pre-checkpoint main/WAL/SHM bytes, checkpointed bytes per game, and the
healthy concurrency lower bound established by that run.

Run human-human and human-agent profiles separately on the intended host. A
successful target is an evidence-backed lower bound for that exact fixture,
pairing, duration, build mode, and host—not a universal maximum. For a hosting
decision, increase `--sessions` between acceptance runs until the first failed
correctness gate or unacceptable latency/resource threshold, then use the last
healthy result with operational headroom. External proxy/TLS, browser, actual
game logic, model/provider quotas, and regional latency require a separate
authorized deployment calibration; the embedded run intentionally cannot
claim to measure them.

## Execute the bundled Wasm fixture

The stress server loads the bundled fixture once and gives each session its own
Wasm instance. The game returns two observations after each accepted turn and
a recovery checkpoint every 32 accepted actions. SQLite, networking, and
audio execute in the production Rust host. To exercise a modified fixture,
rebuild it and pass the component explicitly.

```sh
rust-server-tests/stress-fixture/build-embedded.sh
cargo stress --headless --pairing human-human --sessions 12 --seconds 120 \
  --seed 101 --wasm-module rust-server-tests/stress-fixture/embedded/stress.wasm
```

During steady traffic,
participant A times each nonterminal action from the start of its send until
it reads its matching transition on the game WebSocket. The
`steady_action_to_observation` report gives the matched sample count, unmatched
count, and nearest-rank p50 and p95 in milliseconds. Actions still outstanding
at the end of steady count as unmatched. This elapsed time includes SQLite,
transport, and client scheduling. The server CPU field accumulates time from
startup through cleanup. RSS is sampled after the rooms drain. Repeat paired
runs, alternate their order, and compare differences within each pair.
