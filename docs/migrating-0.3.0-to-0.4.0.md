# Migrating Parlando 0.3.0 to 0.4.0

Parlando 0.4.0 intentionally breaks the game-construction, participant-session,
and SQLite interfaces. Upgrade source code and a copy of the database before
starting a 0.4.0 server against production data.

## Back up and update SQLite

Stop every process using the database. Create a SQLite-consistent backup:

```sh
sqlite3 parlando.sqlite ".backup parlando.sqlite.pre-0.4.0-backup"
sqlite3 parlando.sqlite.pre-0.4.0-backup "pragma integrity_check"
```

The integrity check must print `ok`. Parlando does not perform an automatic
0.3-to-0.4 schema migration. Rename the public session identifier column in
place, then verify the modified database:

```sh
sqlite3 parlando.sqlite \
  "begin immediate;
   alter table sessions rename column room_id to public_session_id;
   update experiments
     set config_json = replace(config_json,
       'waiting_room_timeout_seconds',
       'waiting_session_timeout_seconds');
   update experiment_config_revisions
     set config_json = replace(config_json,
       'waiting_room_timeout_seconds',
       'waiting_session_timeout_seconds');
   update session_events set event_type = 'log'
     where event_type = 'extension_log';
   commit;"
sqlite3 parlando.sqlite "pragma integrity_check"
```

Restore the backup if either command fails. Do not run the upgrade transaction
twice. The two `replace` operations update current and historical experiment
configuration. The final update normalizes logging rows produced by development
builds of the 0.4 API; a stock 0.3 database normally has no such rows.

## Replace game prototypes with a factory

In 0.3.0, `Server::new` accepted a `Game` value. In 0.4.0, every `Game` value
belongs to exactly one session and `Server::new` accepts a separate factory.

Before:

```rust,ignore
#[derive(Default)]
struct MyGame;

impl Game for MyGame {
    // mechanics
}

Server::new(MyGame, metadata)?;
```

After:

```rust,ignore
struct MyGame {
    logger: SessionLogger,
}

struct MyGameFactory;

impl GameFactory for MyGameFactory {
    type Game = MyGame;

    fn create(&self, context: GameSessionContext) -> anyhow::Result<MyGame> {
        Ok(MyGame { logger: context.logger })
    }

    fn validate_config(&self, config: &MyGameConfig) -> anyhow::Result<()> {
        // Move custom validation here from Game::validate_config.
        Ok(())
    }
}

impl Game for MyGame {
    // mechanics only; create_session and validate_config no longer exist here
}

Server::new(MyGameFactory, metadata)?;
```

Do not add a no-session constructor to `MyGame`. Mechanics tests should create
the game through their factory using Parlando's test-only logger support.

## Update agent factories

`AgentContext` now contains `logger: SessionLogger`. Existing factories that
destructure the context must accept the new field. An agent may retain
`context.logger` and call `log` from any method or helper. The runtime assigns
the session, participant, and role; agent code supplies only text.

Remote-agent protocol responses now contain repeated `session_logs` strings.
Regenerate clients from `parlando_agent_v3.proto`, or upgrade the Python SDK and
use its injected `Context.logger`.

## Update participant HTTP and WebSocket clients

The following names change without compatibility aliases:

| 0.3.0 | 0.4.0 |
| --- | --- |
| `POST /api/rooms` | `POST /api/sessions` |
| `/api/rooms/{room_id}/game-session` | `/api/sessions/{public_session_id}/game-session` |
| `/api/rooms/{room_id}/audio-session` | `/api/sessions/{public_session_id}/audio-session` |
| `/api/rooms/{room_id}/voice-diagnostics` | `/api/sessions/{public_session_id}/voice-diagnostics` |
| response/message field `room_id` | `public_session_id` |
| TypeScript `JoinedRoom` | `JoinedSession` |
| TypeScript `roomId` | `sessionId` |

WebSocket endpoint prefixes remain `/ws/game/` and `/ws/audio/`, but their path
parameter and all server-message correlation fields are session identifiers.

## Update experiment configuration

Rename `session.waiting_room_timeout_seconds` to
`session.waiting_session_timeout_seconds` in stored or submitted configuration.
The 0.4.0 server rejects the old key.

## Validate the upgrade

Build the game and participant client, start the server against a disposable
copy of the modified database, and verify participant creation, session entry,
one game transition, administrator session inspection, and a full export before
deploying the upgraded process.
