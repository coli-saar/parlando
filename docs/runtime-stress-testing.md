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
  --preset smoke --pairing human-agent --sessions 2 --seconds 20 --no-tui \
  --report /tmp/parlando-runtime-stress.json
```

Run the interactive dashboard with more sessions:

```sh
cd rust-server-tests
cargo run --release --features stress-tui --bin runtime-stress -- \
  --preset acceptance --pairing human-agent
```

Use `q` or `Esc` to request a clean stop. `Tab` cycles dashboard time-series
views. Use `--keep-database` to preserve the temporary directory for SQLite
inspection; failed runs preserve it automatically. Each run also writes a JSON
report containing the resolved workload, correctness counters, reconnects,
ASR/TTS counts, admission latency, audio scheduler misses, SQLite row counts,
pre-checkpoint main/WAL/SHM bytes, checkpointed bytes per game, and the healthy
concurrency lower bound established by that run.

Run human-human and human-agent profiles separately on the intended host. A
successful target is an evidence-backed lower bound for that exact fixture,
pairing, duration, build mode, and host—not a universal maximum. For a hosting
decision, increase `--sessions` between acceptance runs until the first failed
correctness gate or unacceptable latency/resource threshold, then use the last
healthy result with operational headroom. External proxy/TLS, browser, actual
game logic, model/provider quotas, and regional latency require a separate
authorized deployment calibration; the embedded run intentionally cannot
claim to measure them.
