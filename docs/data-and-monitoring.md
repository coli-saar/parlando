# Data and monitoring

Parlando builds a structured research record while an experiment runs. SQLite
stores ordered game actions, messages, selected transcripts, agent behavior, and
completion outcomes according to the experiment's storage settings. The
dashboard supports live monitoring; the database-backed export is the durable
record for analysis.

## Preserve provenance

Every session records three identifiers that should remain attached during
analysis:

- the experiment id;
- the experiment configuration revision; and
- the exact semantic game version.

The configuration revision identifies the settings in force when the session was
created. Later dashboard edits create another revision rather than rewriting that
history. The game version identifies the compiled state machine that interpreted
the actions.

An experiment from an earlier game version remains inspectable and exportable. It
cannot be activated under a different version. Cloning copies its configuration
into a new experiment for the current version; it does not alter the historical
experiment or its sessions.

## Monitor an experiment

Open `/admin/experiments`. The dashboard header identifies the compiled game and
version, and the catalogue lists every experiment stored for that game process.
Use pinning for experiments that should remain prominent and obsolete status for
entries that should be retained but de-emphasized.

After selecting an experiment, the dashboard provides:

- lifecycle control;
- database-backed configuration and revision history;
- session status and participant roles;
- a process-local Load page for capacity, transport liveness, throughput, and storage;
- a compact event timeline for actions, messages, transcripts, and diagnostics;
- export controls; and
- counted participant-data deletion.

![Experiment dashboard identifying the compiled Space Game version and its
inactive starter experiment](images/parlando-dashboard.jpg)

The dashboard is deliberately game-scoped. Different compiled games use different
processes and dashboards, usually distinguished by port or hostname.

### Interpret load and liveness

The Load page samples the experiment runtime every five seconds and retains at
most one hour in memory. It shows admitted and waiting sessions, unattached
participants, reserved ASR streams, active agents, game/audio connections, HTTP
work, input rates and rejections, audio drops, ASR backpressure, TTS activity,
and the main SQLite, WAL, and shared-memory footprint. The history resets when
the process restarts. It is operational telemetry, not a durable research table,
and it does not appear in exports.

Each runtime session and participant role has a game-transport indicator. `live`
means a heartbeat or game message arrived within five seconds, `delayed` means
five to fifteen seconds, and `stale` means the socket is still registered but has
been quiet for more than fifteen seconds. `Disconnected` means there is no
current game socket. The server still allows up to 90 seconds before closing a
quiet socket, so a brief delayed or stale indication is diagnostic rather than a
declaration that the research session has failed. Audio is shown separately as
connected plus the age of its last frame; a participant may simply be muted, so
quiet audio is not classified as stale.

Heartbeat liveness and meaningful game activity are deliberately separate. A
healthy heartbeat proves that the browser transport is responsive, but it does
not extend the waiting, idle, reconnect, or maximum-lifetime deadline. The
liveness table shows the meaningful-activity age and the earliest applicable
lifecycle deadline so researchers can distinguish a connected but inactive
participant from a lost browser.

### Interpret lifecycle state

An experiment is either `inactive` or `active`. `Active` permits new participant
and room creation. `Inactive` rejects new intake with HTTP 503.

Every process start resets all experiments to `inactive`. Deactivation also leaves
existing room and WebSocket activity running, so it is suitable for closing
recruitment without interrupting participants. It is not a session kill switch.

### Interpret the timeline

The dashboard groups durable rows into a compact operational view. Use it to find
stalled rooms, rejected actions, transcript activity, agent failures, and
completion events. Use an export when analysis depends on complete structured
payloads or exact ordering; the dashboard is a summary, not an alternative data
source.

When `privacy.store_voice_diagnostics` is enabled, playback underruns appear as
`voice_diagnostic` events. They contain an event name, cumulative connection-local
count, and remaining buffered source samples. Parlando discards device ids, device
labels, user agents, and arbitrary browser error messages. Sustained underruns may
justify increasing `voice.jitter_buffer_ms`, but a larger buffer also delays
playback startup.

## Choose what to retain

Four experiment-level privacy switches let researchers collect the data their
analysis requires without retaining every available stream. They independently
control full game state, typed messages, final transcripts, and minimized voice
diagnostics. The dashboard displays the effective choices, and disabled data
types are not written to SQLite or included in later exports.

Other durable records can include:

- participant creation and role assignment;
- consent declarations and participant-information evidence;
- accepted and rejected actions;
- accepted transitions, with their action, generated events, and resulting state stored once;
- typed and final spoken conversation messages, each stored once with modality metadata;
- agent startup, proposals, messages, and errors; and
- TTS and voice diagnostics.

Rejected actions use bounded analytical records rather than retaining arbitrary
attacker-controlled payloads. Each record includes a stable `reason_code`, a
bounded error description, and—when an action body existed—its byte count and
SHA-256 fingerprint. Repeated equivalent violations are coalesced into bounded
per-minute aggregates so rejection monitoring cannot itself fill the database.

Completion summaries are game-specific. Define them to contain the terminal
outcome, score or condition labels, and any final task state required by the
analysis. Do not rely on reconstructing an essential outcome solely from browser
presentation.

### Research identifiers

Parlando assigns readable three-word identifiers to human participants and
dialogues. Human identifiers end in an animal noun; dialogue identifiers end in a
place or object noun. These identifiers remain stable across repeated exports of
the same experiment.

Human identifiers are experiment-scoped. The same recruited person receives an
independent identifier in another experiment. Agent identifiers instead record
their type, implementation name, and version, for example
`agent:space_game.back_and_forth:BackAndForthAgent@0.2.0`.

These labels support pseudonymous analysis without carrying a name or recruitment
identifier into normal exports. Data remain pseudonymous while an external
recruitment mapping or identifying dialogue content can still link them to a
person.

## Choose an export

The dashboard export action calls
`/api/admin/runtime/{experiment_id}/export`. It reconstructs data from SQLite and
supports JSON, YAML, and CSV encodings.

Use the variants as follows:

| Variant | Intended use | Deliberate boundary |
| --- | --- | --- |
| `research` | Normal pseudonymous analysis | Omits recruitment identity, consent evidence, and internal runtime identifiers |
| `corpus` | Preparation for a dialogue-data release | Removes additional internal identifiers and absolute timestamps, but remains a `corpus_candidate` requiring linkage removal and content review |
| `full` | Restricted administration, audit, or recovery | Contains internal administrative records and should not be used as the ordinary analysis export |

Both public-facing variants retain the experiment-specific participant and
dialogue labels so repeated exports can be joined. They also retain session game
version and configuration revision. New internal database fields do not
automatically enter the fixed research and corpus projections.

Use export data for task success, action sequences, utterance counts, timing,
agent comparisons, transcription diagnostics, and exclusion decisions.

## Delete participant data

Human participant cards provide a preview and confirmed deletion action. Deletion
removes consent evidence plus authored messages and transcripts, clears direct
identity fields, and removes participant references where this can be done without
destroying the shared fictitious game record. Shared actions needed to interpret
the other participant's session remain with a deleted-participant marker.

Parlando performs no automatic retention deletion. The institution must define
retention, backups, and treatment of data already exported or released.

## Review the installation privacy status

`/admin/privacy` reports the effective Privacy Contract version, storage switches,
external speech services, export capabilities, deletion support, and consent
evidence settings. It can be downloaded as Markdown or JSON.

This report makes the platform's technical behavior inspectable and downloadable.
The institution adds the decisions that software cannot infer: controller,
legal basis, provider contracts, retention policy, DPO approval, and the release
assessment for a corpus candidate.

## Operational checklist

- Back up the SQLite file before migration, deletion, or redeployment.
- Put hosted deployments on persistent storage; the Render example uses
  `sqlite:////data/parlando.sqlite` on a 10 GB disk.
- Record agent type and version so exports distinguish experimental policies.
- Treat process health and active intake as separate monitoring signals.
- Protect administrator routes at the deployment boundary as defense in depth;
  Parlando still requires its own authenticated session, roles, and CSRF tokens.
