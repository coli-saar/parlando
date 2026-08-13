# Parlando Evaluation Database Schema

## Scope

Parlando stores experiment data for clean evaluation, not as a generic audit log. The durable keys are `experiment_id` and `session_id`; existing client-facing values such as `room_id` and `participant_session_id` remain aliases/handles.

The reusable server talks to a backend-neutral `ExperimentStore` trait. SQLite is the first implementation, selected with `database.url = sqlite:///...`.

## Tables

### `experiments`

One row per configured or generated experiment run.

```sql
experiment_id text primary key
created_at text not null
config_json text not null
server_version text
notes text
```

The experiment id comes from CLI `--experiment-id`, then YAML `experiment.id`, then a generated startup id.

### `participants`

One row per durable participant identity.

```sql
participant_id integer primary key autoincrement
participant_kind text not null
identity_provider text not null
external_id text
display_name text
metadata_json text
created_at text not null
```

`participant_kind` is values such as `human`, `agent`, or `worker`. `identity_provider` is values such as `prolific`, `direct`, `agent`, or `worker`.

There is a partial unique index on `(identity_provider, external_id)` when `external_id is not null`, so returning Prolific users can map to the same participant row while direct users without stable external ids can create fresh rows.

### `sessions`

One row per game instance.

```sql
experiment_id text not null
session_id integer not null
room_id text not null unique
mode text not null
status text not null
created_at text not null
started_at text
completed_at text
completion_json text
primary key (experiment_id, session_id)
```

`session_id` counts upward from `1` inside each experiment. `room_id` is the client-facing room alias.

### `session_participants`

One row per participant appearance in a session.

```sql
experiment_id text not null
session_id integer not null
participant_id integer not null
participant_session_id text not null unique
role text not null
joined_at text not null
left_at text
connection_status text not null
primary key (experiment_id, session_id, participant_id)
```

Roles such as `A`, `B`, and `worker` are session-local. They do not belong on `participants`. Additional player joins are rejected once `A` and `B` are occupied.

`participant_session_id` is a client/reconnect handle. It is not durable identity.

### `consent_declarations`

One row per item-level consent declaration.

```sql
consent_id integer primary key autoincrement
experiment_id text not null
session_id integer
participant_id integer not null
consent_item_id text not null
accepted integer not null
declared_at text not null
consent_text_hash text
metadata_json text
```

Consent is separate from participants because declarations are timestamped and may be session-specific.

### `session_events`

One row per evaluation-relevant occurrence inside a session.

```sql
event_id integer primary key autoincrement
experiment_id text not null
session_id integer not null
event_index integer not null
event_type text not null
actor_participant_id integer
actor_role text
payload_json text not null
game_state_json text
created_at text not null
unique (experiment_id, session_id, event_index)
```

Indexes:

```sql
create index idx_session_events_session
    on session_events(experiment_id, session_id);

create index idx_session_events_session_type
    on session_events(experiment_id, session_id, event_type);

create index idx_session_events_actor_created
    on session_events(actor_participant_id, created_at);
```

The event stream is not duplicated into category tables. It is the ordered reconstruction surface for evaluation.

## Current Event Types

The intended event vocabulary includes:

- `session_created`
- `participant_joined`
- `participant_connected`
- `participant_disconnected`
- `ready`
- `game_action_submitted`
- `game_action_accepted`
- `game_action_rejected`
- `state_changed`
- `transcript_segment`
- `conversation_message`
- `voice_diagnostic`
- `agent_started`
- `agent_action`
- `agent_error`
- `tts_diagnostic`
- `session_completed`

Accepted game actions store the actor, typed action payload, game events, full resulting game state, and timestamp.

The `voice_diagnostic` payload may include browser `audio_playback_underrun` events with cumulative underrun and buffered-sample measurements. Raw PCM is not stored in the event stream.

## Runtime Cache Boundary

The current server still keeps active participants, rooms, broadcasts, transcript buffers, and conversation buffers in memory for live WebSocket/game execution. Durable semantic state should be written through `ExperimentStore`; caches are allowed only for active runtime coordination and should be recoverable or replaceable in later work.
