# Data And Monitoring

Parlando records study data as durable session events in SQLite. The event stream is intended to support later analysis without relying on active runtime memory or browser-local state.

For researchers, the important rule is simple: analyze the export, not what a browser happened to show at the end of a session. The export is reconstructed from persisted rows and includes the ordered history needed to recover actions, messages, transcripts, agent behavior, and completion outcomes.

## Terms

- **Game** means the binary/client/adapter/agent implementation being run.
- **Experiment** means one data-collection campaign using that game.
- **Session** means one play-through inside an experiment.

## Persisted Data

Parlando stores durable study data through the reusable `ExperimentStore` abstraction. The current implementation uses SQLite.

Depending on the four `privacy` switches, event categories can include:

- participant joins and consent declarations.
- accepted game actions.
- state changes.
- completion summaries.
- transcript segments.
- conversation messages.
- agent startup, action proposals, and errors.
- TTS and voice diagnostics.

When `privacy.store_voice_diagnostics` is enabled, browser playback underruns are persisted as `voice_diagnostic` events with event name `audio_playback_underrun`, a cumulative connection-local count, and the remaining buffered source-sample count. Device identifiers, device labels, user agents, and free error messages are discarded. These events are useful for deciding whether a deployment needs a larger `voice.jitter_buffer_ms`; do not increase the buffer solely from anecdotal latency reports because it directly delays playback startup.

Room ids and participant-session ids are useful during live play. At creation time, Parlando also assigns three-word identifiers to human participants and dialogues. Human participant identifiers end in an animal noun; dialogue identifiers end in a place or object noun. Agent participants instead receive a descriptive identifier such as `agent:space_game.back_and_forth:BackAndForthAgent@0.2.0`, built from their configured type, implementation name when available, and version. These identifiers remain unchanged across repeated exports of the same experiment so separately produced datasets can be joined. Human participant identifiers are experiment-scoped: the same recruitment identity receives an independently generated identifier in each experiment.

Completion summaries are game-specific. Design them deliberately: include success or failure, score or outcome labels, final task state needed for interpretation, and any condition labels required by downstream scripts.

## Operator Monitor

Use `/admin/experiments` for quick inspection during a study run. The dashboard reads from the database, lists experiments and their sessions, and shows a compact session timeline of actions, conversation messages, and transcripts. The older `/admin/games` route remains as a compatibility alias.

The dashboard intentionally summarizes event rows. Use export when you need full structured payloads.

The global dashboard header links to `/admin/privacy`, an installation-wide privacy status derived from the running server version and effective configuration. The page is protected by the same administrator session as the experiment dashboard and offers Markdown and JSON downloads. It reports the configured Privacy Contract version, effective storage switches, external services, exports, deletion support, and consent-evidence configuration. It does not claim that a DPO has approved the installation.

## Export

Use `/api/admin/export` to retrieve export data reconstructed from persisted rows, not active in-memory rooms. `variant=research` is the default and contains pseudonymized research data without recruitment identity or consent evidence. `variant=corpus` produces a publication-oriented `corpus_candidate`, not an automatically anonymous corpus. Both variants retain the experiment-specific readable participant and dialogue identifiers while omitting internal database, room, session, and participant-session identifiers. `variant=full` is the internal administrative export. JSON, YAML, and CSV encodings remain available.

The export is the right source for downstream scripts that compute task success, timing, utterance counts, action sequences, agent metadata, transcription diagnostics, or exclusion flags.

## Practical Notes

- Treat the SQLite file as study data. Back it up before deleting or redeploying a service.
- On Render, use a persistent disk for `/data` and configure `database.url` as `sqlite:////data/parlando.sqlite`.
- Record agent names and versions in config so exported participant metadata can distinguish policies.
- Keep deployment-level access controls as defense in depth; Parlando also requires an authenticated administrator session, role checks, and CSRF tokens on mutations.
- Human participant cards in a session show their experiment-specific random participant identifier and provide a counted, confirmed deletion action. Parlando deliberately performs no automatic retention deletion.
