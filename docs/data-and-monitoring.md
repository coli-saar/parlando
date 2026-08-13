# Data And Monitoring

Parlando records study data as durable session events in SQLite. The event stream is intended to support later analysis without relying on active runtime memory or browser-local state.

For researchers, the important rule is simple: analyze the export, not what a browser happened to show at the end of a session. The export is reconstructed from persisted rows and includes the ordered history needed to recover actions, messages, transcripts, agent behavior, and completion outcomes.

## Terms

- **Game** means the binary/client/adapter/agent implementation being run.
- **Experiment** means one data-collection campaign using that game.
- **Session** means one play-through inside an experiment.

## Persisted Data

Parlando stores durable study data through the reusable `ExperimentStore` abstraction. The current implementation uses SQLite.

Important event categories include:

- participant joins and consent declarations.
- accepted game actions.
- state changes.
- completion summaries.
- transcript segments.
- conversation messages.
- agent startup, action proposals, and errors.
- TTS and voice diagnostics.

Browser playback underruns are persisted as `voice_diagnostic` events with event name `audio_playback_underrun`, a cumulative connection-local count, and the remaining buffered source-sample count. These events are useful for deciding whether a deployment needs a larger `voice.jitter_buffer_ms`; do not increase the buffer solely from anecdotal latency reports because it directly delays playback startup.

Room ids and participant-session ids are useful during live play. Evaluation data also has durable experiment, session, and participant identifiers so analysis does not depend on transient browser connections.

Completion summaries are game-specific. Design them deliberately: include success or failure, score or outcome labels, final task state needed for interpretation, and any condition labels required by downstream scripts.

## Operator Monitor

Use `/admin/experiments` for quick inspection during a study run. The dashboard reads from the database, lists experiments and their sessions, and shows a compact session timeline of actions, conversation messages, and transcripts. The older `/admin/games` route remains as a compatibility alias.

The dashboard intentionally summarizes event rows. Use export when you need full structured payloads.

## Export

Use `/api/admin/export` to retrieve JSON export data for analysis. The export is reconstructed from persisted rows, not active in-memory rooms.

The export is the right source for downstream scripts that compute task success, timing, utterance counts, action sequences, agent metadata, transcription diagnostics, or exclusion flags.

## Practical Notes

- Treat the SQLite file as study data. Back it up before deleting or redeploying a service.
- On Render, use a persistent disk for `/data` and configure `database.url` as `sqlite:////data/parlando.sqlite`.
- Record agent names and versions in config so exported participant metadata can distinguish policies.
- Add deployment-level access controls before exposing admin routes outside trusted environments.
