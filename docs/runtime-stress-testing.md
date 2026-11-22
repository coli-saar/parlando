# Runtime Stress Testing

`runtime-stress` drives a real loopback Parlando server through public HTTP,
game-WebSocket, and audio-WebSocket endpoints. It uses a temporary file-backed
SQLite database and the same Ratatui dashboard as `audio-stress-tui`.

Each run creates one experiment. Choose its communication mode with
`--communication typed` or `--communication speech` (the default), and its
seats with `--pairing human-human` or `--pairing human-agent`. In each
combination the harness admits participants, opens game sockets, plays the
bundled game, reconnects a subset of rooms, and submits a finishing move.

| Communication | Human–human | Human–agent |
| --- | --- | --- |
| Typed | Both seats send text through the game WebSocket. The harness checks that each message reaches the intended room and partner. | The human sends a room-specific message and receives the agent's reply through the same socket. |
| Speech | Both seats send PCM frames every 20 ms. The harness verifies relayed frames byte-for-byte. | Local recognition turns human audio into text for the agent. Local synthesis turns its replies into audio that the harness verifies on the participant's audio socket. |

Typed runs require only game sockets. Speech runs start deterministic local
provider processes and need no external credentials.

Every run has four phases in fixed 1:4:3:2 proportions:

| Phase | Acceptance duration | Behavior |
| --- | ---: | --- |
| Ramp | 1 minute | Pace admissions up to target concurrency |
| Steady | 4 minutes | Sustain game actions and the selected communication workload |
| Churn | 3 minutes | Reconnect the game socket for a deterministic quarter of sessions and reconnect the audio socket in speech mode |
| Drain | 2 minutes | Stop traffic, send terminal actions, close sockets, and collect evidence |

The same proportions are retained for shortened runs, so a 60-second smoke run
uses 6/24/18/12 seconds. The minimum override is 10 seconds.

Run a short typed-chat smoke test from the repository root:

```sh
cargo stress --communication typed --pairing human-agent \
  --sessions 4 --seconds 20 --headless
```

To exercise the whole matrix, run the four conditions consecutively. This
keeps each condition's traffic and measurements in its own report.

```sh
make test-runtime-communication
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

For typed runs, compare `workload.typed_messages_sent`,
`workload.typed_messages_verified`, and `database.typed_messages`. A passing
run reconciles all three, checks a two-way exchange in every room, and
verifies that every game completed. In human–agent runs,
`workload.typed_agent_replies_verified` counts replies matched to a specific
human message. The agent's initial greeting also appears in
`database.agent_messages`. Typed runs require zero speech events and zero
final transcripts in SQLite.

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
