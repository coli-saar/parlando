# Parlando configuration and deployment reference

A generated game process hosts one compiled game and any number of dashboard-managed experiments. Researchers create, clone, activate, and edit experiments at `/admin/experiments`. Do not generate Rust access to the complete experiment configuration and do not make YAML the normal authoring surface.

## Ownership

The process receives only bootstrap coordinates needed before the dashboard opens:

- listener host and port;
- SQLite database URL;
- optional compiled participant-application directory;
- optional public origin; and
- provider secrets through private environment or secret storage.

`Game::Config` contains only game-mechanics settings. An agent factory receives only its selected settings. The dashboard/runtime owns participant information and consent copy, access, pairing, capacity, lifecycle limits, privacy, conversation, agents, voice, transcription, and TTS. There is no configurable participant title; the standard application uses functional screen headings and game presentation belongs to the frontend.

Do not model a Parlando `Study`: the runtime hierarchy is Game, Experiment, Session. A broader research study is external.

## Voice

Voice, transcription, and TTS are runtime capabilities, not game decisions. Generated participant applications read narrow `GameSession` capability/status fields and use SDK widgets. Generated servers and clients must not implement audio routes, media framing, provider clients, browser STT/TTS, or credential transport.

Keep `SPEECHMATICS_API_KEY`, `ELEVENLABS_API_KEY`, and any remote-agent model credentials out of source, frontend variables, experiment exports, and checked-in files. Administrators configure non-secret behavior through the dashboard.

## Local run

A generated project should offer one run path, for example:

```sh
<game-binary> \
  --host 127.0.0.1 \
  --port 8000 \
  --database-url sqlite:///.local/parlando.sqlite \
  --client-dist client/dist
```

Useful routes are `/health`, `/admin/experiments`, and the experiment-scoped participant route `/e/{experiment_id}/`.

## Container deployment

Build the participant application, build/install the Rust binary, copy the frontend assets to a stable image path, and start one server process with its database on durable storage. Set bootstrap coordinates with CLI arguments or environment variables supported by the generated binary.

For Render or another platform, mount persistent storage for SQLite and keep provider keys in platform secrets. Use one Parlando process unless sticky routing and process-local live-session ownership have been designed explicitly; audio rooms and one-use credentials are process-local today.

## Handoff

Tell the user the exact run command, dashboard URL, database path, participant asset path, required secret variables, and any remote-agent process command. Tell researchers to create and configure experiments in the dashboard instead of editing generated game code.
