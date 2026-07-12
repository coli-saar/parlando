# Data And Monitoring

Parlando records study data as durable session events in SQLite. The event stream is intended to support later analysis without relying on active runtime memory or browser-local state.

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

Room ids and participant-session ids are useful during live play. Evaluation data also has durable experiment, session, and participant identifiers so analysis does not depend on transient browser connections.

## Operator Monitor

Use `/admin/experiments` for quick inspection during a study run. The dashboard reads from the database, lists experiments and their sessions, and shows a compact session timeline of actions, conversation messages, and transcripts. The older `/admin/games` route remains as a compatibility alias.

The dashboard intentionally summarizes event rows. Use export when you need full structured payloads.

## Export

Use `/api/admin/export` to retrieve JSON export data for analysis. The export is reconstructed from persisted rows, not active in-memory rooms.

The export is the right source for downstream scripts that compute task success, timing, utterance counts, action sequences, agent metadata, or transcription diagnostics.

## Practical Notes

- Treat the SQLite file as study data. Back it up before deleting or redeploying a service.
- On Render, use a persistent disk for `/data` and configure `database.url` as `sqlite:////data/parlando.sqlite`.
- Record agent names and versions in config so exported participant metadata can distinguish policies.
- Add deployment-level access controls before exposing admin routes outside trusted environments.
