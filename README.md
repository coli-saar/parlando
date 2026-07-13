# Parlando

Parlando helps researchers build and run browser-based dialogue game experiments. It gives you reusable infrastructure for rooms, roles, WebSockets, voice, agents, persistence, monitoring, and export, while keeping the actual game mechanics under your control.

Use it when the object of study is an interactive task carried by dialogue: participants see role-specific information, act in a shared world, coordinate through text or speech, and leave behind a structured record of what happened.

Parlando is currently aimed at research teams that want to prototype, deploy, and analyze controlled dialogue games without rebuilding the surrounding study infrastructure every time.

## What You Can Study With It

Parlando is a good fit for experiments where the task state and the conversation shape each other. Examples include:

- asymmetric-information coordination tasks where each participant sees a different view of the same world.
- map tasks, reference games, repair games, negotiation games, and collaborative puzzles.
- human-vs-human and human-vs-agent comparisons using the same game rules.
- studies that need text chat, speech, transcript data, typed game actions, and task outcomes in one export.
- controlled pilot studies where researchers want a custom task UI but do not want to write room, reconnect, consent, admin, and export infrastructure from scratch.

Parlando is less useful for single-participant surveys, reaction-time tasks, or round-based economic games where live shared state and dialogue are not central.

## What Parlando Provides

- Run two-player dialogue games in the browser with server-owned room creation, role assignment, reconnect handling, and waiting-room readiness.
- Keep game state authoritative in Rust, with typed actions, observations, validation, transition events, and completion summaries.
- Let each player receive a role-specific view of the same underlying game, which is useful for asymmetric information, coordination tasks, repair games, map tasks, reference games, and other interactive dialogue settings.
- Provide a reusable browser protocol and JavaScript client package for participants, room setup, WebSockets, audio-session setup, and typed game payloads.
- Support human-vs-human and human-vs-agent studies from the same experiment configuration.
- Include infrastructure for direct studies, matchmaking, and Prolific-oriented experiment automation.
- Provide optional voice infrastructure through LiveKit partner audio, Speechmatics browser transcription, ElevenLabs text-to-speech, and LiveKit agent audio publishing.
- Persist evaluation data in SQLite: experiments, durable participants, sessions, consent declarations, ordered actions, state changes, transcripts, conversation messages, agent events, and diagnostics.
- Serve a bundled browser client when a built frontend is available, while keeping API and WebSocket routes usable for separately deployed clients.
- Expose a DB-backed operator monitor at `/admin/experiments` and JSON export at `/api/admin/export`.
- Connect custom agents when needed, either inside the Rust game crate or as Python services over gRPC.
- Deploy as a Docker web service, with `space-game/render.yaml` and a Render-safe example config included.

## The Core Idea

A Parlando game is a small Rust adapter around your experiment logic:

```rust
impl GameAdapter for MyGameAdapter {
    type State = MyState;
    type Action = MyAction;
    type Observation = MyObservation;
    type Event = MyEvent;
    type Summary = MySummary;

    fn initial_state(&self) -> Self::State {
        initial_state()
    }

    fn validate_action(&self, state: &Self::State, action: &Self::Action, player: PlayerRole) -> anyhow::Result<()> {
        validate_action(state, action, player)
    }

    fn apply_action(&self, state: &Self::State, action: &Self::Action) -> anyhow::Result<Self::State> {
        apply_action(state, action)
    }

    fn observe_state(&self, state: &Self::State, player: PlayerRole) -> Self::Observation {
        observe_state_for_player(state, player)
    }

    fn events_for_action(&self, before: &Self::State, after: &Self::State, action: &Self::Action, player: PlayerRole) -> Vec<Self::Event> {
        events_visible_to_player(before, after, action, player)
    }

    fn is_complete(&self, state: &Self::State) -> bool {
        state.finished
    }

    fn completion_summary(&self, state: &Self::State) -> Self::Summary {
        summarize(state)
    }
}
```

That adapter is enough for the reusable server to handle rooms, WebSockets, persistence, export, and agents. The game crate stays typed; JSON appears only at the browser, database, and remote-agent boundaries.

The planned Parlando LLM skill will make this workflow more accessible: experiment authors should be able to describe the game, state, actions, and interface they want, then use the skill to generate and revise the Rust adapter and JavaScript client. Understanding Rust or JavaScript will still help for review and debugging, but it should not be a prerequisite for starting a Parlando-based game.

## Compared With Other Platforms

Parlando sits near existing experiment systems, but it is optimized for a narrower design point: interactive dialogue games with authoritative state, role-specific views, voice/conversation data, and deployable evaluation records.

| Platform | Good fit | How Parlando differs |
| --- | --- | --- |
| [Slurk](https://github.com/clp-research/slurk) | Dialogue experiments centered on typed chat, with rooms, bots, layouts, and optional JavaScript task components. | Parlando puts the JavaScript game itself in the foreground and treats dialogue as the communication layer around that game. Voice is designed to stay out of the way of the task UI, while the server owns typed state transitions, readiness, persistence, export, and a YAML setup story for local and deployed studies. |
| [Empirica](https://docs.empirica.ly/) | Multiplayer browser experiments and games with an admin panel, batches, treatments, and React/JavaScript app structure. | Parlando is less general-purpose but more focused on dialogue game data: role-specific observations, conversation/transcript persistence, voice-session planning, and Rust-authoritative state transitions are core concepts rather than app-level conventions. |
| [oTree](https://www.otree.org/) | Behavioral and economic experiments, especially structured rounds, surveys, games, and participant payments. | Parlando targets continuous interactive dialogue sessions rather than page/round-oriented studies. It emphasizes live WebSocket state, voice/chat traces, custom game clients, and game-specific observations. |
| [Dallinger](https://dallinger.readthedocs.io/) | Crowd-sourced behavioral experiments with recruitment automation, networks, bots, and data management. | Parlando's planned Prolific automation covers the recruitment/setup path for dialogue studies, while the core runtime stays focused on the live game: room readiness, typed actions, state changes, voice/chat traces, transcripts, conversations, and export. |
| [jsPsych](https://www.jspsych.org/latest/) and [lab.js](https://lab.js.org/) | Browser-based single-participant tasks, surveys, timing-sensitive stimuli, and online study building. | Parlando is for multi-participant or human-agent interaction where a server must coordinate shared state, private views, live communication, and durable session events. |

The tradeoff is deliberate. If your study is a conventional survey, reaction-time task, or round-based economic game, one of the established platforms may be a better fit. Parlando is meant for cases where the dialogue and the evolving shared task state are the experiment.

## Agents When You Need Them

Agents are important but not required. A Parlando study can be human-vs-human, human-vs-agent, or both across different configurations. Python agents can connect without writing gRPC code:

```python
from parlando_agent_sdk import AgentResponse, GameAgent, serve_agent

class FirstAvailableAction(GameAgent):
    async def maybe_act(self, available_actions):
        if available_actions:
            return AgentResponse.action_with_message(
                available_actions[0],
                "I will try the first available move.",
            )
        return None

serve_agent(FirstAvailableAction, host="127.0.0.1", port=50051)
```

Point an experiment config at that service with `agents.human_vs_agent.factory: remote_grpc`, and the Rust server treats the Python process like a participant. Returned actions are still validated by the game server before they change state.

## Repository Layout

- `rust-server`: reusable Rust runtime for config, rooms, WebSockets, persistence, audio-session planning, TTS, agent execution, remote gRPC agents, admin views, and export.
- `space-game`: demo Space Game browser app, Rust server binary, adapter, configs, and deployment files.
- `js-client`: reusable TypeScript browser runtime for setup, HTTP helpers, WebSockets, audio-session setup, and React startup components.
- `rust-server/python/parlando-agent-sdk`: Python wrapper for remote gRPC agents.
- `space-game/config/experiment.render.example.yaml`: deployable example experiment config with an optional Render secret-file overlay.
- `space-game/render.yaml`: Render web-service example.
- `docs/`: GitHub-rendered technical documentation for architecture, games, browser protocol, agents, deployment, and data export.

The Space Game browser app lives in `space-game/client`. The Rust server can serve a built client directory through `server.client_dist_path`.

## System Requirements

Parlando builds a Rust server and a TypeScript browser client. Install these tools before running the demo or packaging a game:

- Rust stable toolchain with Cargo. The repository does not pin a `rust-toolchain` file, so use a current stable Rust installation from `rustup`.
- Node.js `20.19.0` or newer, or Node.js `22.12.0` or newer. The Space Game browser build uses Vite 7, whose published engine requirement excludes older Node 20 and early Node 22 releases. Use the matching `npm` that ships with that Node version.
- GNU Make. The top-level and Space Game workflows are documented as `make` targets.
- macOS only: Apple Command Line Tools for Xcode `16.0.0` or newer. Check the selected compiler with `xcrun clang --version`; it should report Apple clang `16.0.0` or newer. For a Command Line Tools-only install, `pkgutil --pkg-info=com.apple.pkg.CLTools_Executables` also reports the installed CLT package version. If the selected developer directory is stale, update the Command Line Tools in System Settings or install a current Xcode, then run `sudo xcode-select --switch /Library/Developer/CommandLineTools` or select the matching Xcode developer directory. This requirement matters for `make install-server`, because the Rust server links native LiveKit/WebRTC dependencies through Apple's compiler and SDK.
- Linux only: a standard C/C++ build toolchain and system libraries sufficient for Rust crates with native dependencies. The included Dockerfile uses Debian Bookworm as the reference Linux build environment.

## Run The Demo Server

Build and test the reusable Rust server crate:

```sh
cd rust-server
cargo test
cd ..
cargo run --manifest-path space-game/server/Cargo.toml -- --host 127.0.0.1 --port 8000
```

With no config file, the server starts with conservative defaults and API routes enabled. For a configured study, pass a YAML file:

```sh
cargo run --manifest-path space-game/server/Cargo.toml -- \
  --host 127.0.0.1 \
  --port 8000 \
  --config space-game/config/experiment.render.example.yaml
```

The main service endpoints are:

- `GET /health`
- `GET /api/config`
- `POST /api/participants`
- `POST /api/direct/start` and `POST /api/direct/enter`
- `POST /api/rooms`, `POST /api/rooms/{room_id}/join`
- `GET /ws/game/{room_id}?participantSessionId=...`
- `GET /admin/experiments`
- `GET /api/admin/export`

## Learn More

Start with [the documentation index](docs/README.md) for architecture, game design, browser protocol, custom agents, local development, Render deployment, and data export. The demo implementation in `space-game/server` shows a complete game adapter and agent selector; `space-game/client` shows the matching participant UI.
