# Migrate from Parlando 0.6.0 to 0.7.0

Upgrade the server, game tooling and browser assets together. Version 0.7.0 runs games as WebAssembly components and makes the participant communication channel an explicit experiment setting. Preserve the old deployment and its database backup so that you can reproduce historical sessions with their original game build.

## Build the game and browser client

Use the 0.7.0 versions of `parlando` and `@coli-saar/parlando-client`. Use matching versions of the Rust game SDK and derive crate, or the Python game and agent SDKs, wherever your project depends on them. For an unreleased candidate, use the candidate artifacts. After publication, install the coordinated versions from their registries.

Start a reference project with `parlando new <game-id> --lang rust` or `--lang python`. Move game mechanics into the session-owned component. For Rust games the public `Game` trait lives in `parlando_game_sdk`. Projects using the earlier component SDK should rename `parlando-component-sdk` to `parlando-game-sdk` and replace `ComponentWire` and `ComponentDecode` with the `GameValue` derive. Build for the v4 game world. Custom Rust hosts load games with `LoadedGame`. Local agents receive `ComponentValue` observations, and Rust reward functions receive their configuration once through `configure`. See the [component interface](component-games.md) and [agent integration guide](agents.md).

Run `parlando test` and `parlando build`. Pass the ZIP path printed by the build to `parlando check --package`. Deploy the resulting ZIP to your host. Retain the exact component used to record each session you wish to replay. See [package a game](web/content/manual/build/packages.md).

## Deploy and activate the game

Start the server with `parlando serve --store DIRECTORY`. Sign in at the
administrator origin and deploy the ZIP from **Games**. Activate the game, then
click its ID to reach the game dashboard. Create and configure your experiment
there. You can also use `parlando deploy` and `parlando activate` with the
corresponding CLI credentials. See the
[local pilot walkthrough](web/content/manual/start/getting-started.md#start-a-local-server)
and the [shared host guide](web/content/manual/deploy/host.md).

## Select the communication mode

Replace `voice.enabled: true` with the following setting. For a previous false or absent flag, use `typed`. Remove the old `enabled` field from `voice` and keep the remaining transport and provider settings.

```yaml
communication:
  mode: speech
```

Typed mode displays the transcript and composer. The server issues no audio ticket and keeps Speechmatics and ElevenLabs inactive. Speech mode displays the meter and mute control. A human-authored text frame receives the nonfatal `typed_messages_disabled` error. Both dashboards retain the speech settings while the operator selects Typed. For agents, typed messages and final transcripts still reach the same speaker-and-text callback.

Rebuild participant assets against the updated client even if you keep the default layout. Custom clients should read `communication.mode` from `GET /api/config` in place of `voice.enabled`. The admin replay response also supplies `communication.mode`, resolved from the session's recorded revision.

Games using the standard `ParticipantApp` get the correct widget by default. To move it, set `communicationPlacement="game"` and render one `CommunicationWidget` from `@coli-saar/parlando-client/react` inside `renderGame`. Remove any game-authored composer or microphone control that would duplicate it. The library includes the CSS and behavior. See the [placement and styling example](web/content/manual/build/create-game.md#place-communication-beside-the-game).

## Convert a populated schema-16 experiment database

Stop the host. Identify the actual SQLite file from the server launch configuration. With `lsof` or your platform’s equivalent, confirm that no process holds the database or its WAL or SHM sidecars. Back up the entire stopped store for a shared host. The commands below apply to Parlando experiment databases whose latest entry in `schema_migrations` is 16. The shared dashboard account database has its own schema.

Create a new backup **beside the database**. From its directory, substitute the actual filenames below. Use a fresh backup name, restrict its permissions, and verify it before continuing.

```sh
umask 077
test -f study.sqlite && test ! -e study.schema16.backup &&
  sqlite3 study.sqlite ".backup 'study.schema16.backup'"
sqlite3 study.schema16.backup 'SELECT max(version) FROM schema_migrations; PRAGMA integrity_check; PRAGMA foreign_key_check;'
```

The expected results are `16`, `ok`, and no foreign-key rows. Record row counts from the backup for `experiments`, `experiment_config_revisions`, `sessions`, and `session_events`.

Run the following in the SQLite CLI on the original file. Use a fresh CLI process with the block as script input. A failed guard stops the script and the connection closes with the transaction rolled back. The conversion preserves all settings except the superseded flag. Previously, speech-enabled sessions could also accept typed messages, so the historical events must retain their original attribution.

```sql
.bail on
PRAGMA foreign_keys = ON;
BEGIN IMMEDIATE;
CREATE TEMP TABLE migration_guard (ok INTEGER NOT NULL CHECK (ok = 1));
INSERT INTO migration_guard SELECT max(version) = 16 FROM schema_migrations;
CREATE TEMP VIEW all_configs AS
  SELECT config_json FROM experiments
  UNION ALL SELECT config_json FROM experiment_config_revisions;
INSERT INTO migration_guard SELECT NOT EXISTS (
  SELECT 1 FROM all_configs WHERE json_type(config_json) <> 'object'
    OR json_type(config_json, '$.communication') IS NOT NULL
    OR coalesce(json_type(config_json, '$.voice'), 'object') <> 'object'
    OR coalesce(json_type(config_json, '$.voice.enabled'), 'false') NOT IN ('true', 'false')
);
UPDATE experiments SET config_json = json_set(
  json_remove(config_json, '$.voice.enabled'), '$.communication',
  json_object('mode', CASE WHEN json_extract(config_json, '$.voice.enabled') THEN 'speech' ELSE 'typed' END)
);
UPDATE experiment_config_revisions SET config_json = json_set(
  json_remove(config_json, '$.voice.enabled'), '$.communication',
  json_object('mode', CASE WHEN json_extract(config_json, '$.voice.enabled') THEN 'speech' ELSE 'typed' END)
);
INSERT INTO migration_guard SELECT NOT EXISTS (
  SELECT 1 FROM all_configs WHERE json_type(config_json, '$.voice.enabled') IS NOT NULL
    OR coalesce(json_extract(config_json, '$.communication.mode'), '') NOT IN ('typed', 'speech')
);
INSERT INTO migration_guard SELECT NOT EXISTS (
  SELECT 1 FROM sessions s LEFT JOIN experiment_config_revisions r
    ON r.experiment_id = s.experiment_id AND r.revision = s.config_revision
  WHERE r.revision IS NULL
);
INSERT INTO migration_guard SELECT NOT EXISTS (
  SELECT 1 FROM experiments e LEFT JOIN experiment_config_revisions r
    ON r.experiment_id = e.experiment_id AND r.revision = e.config_revision
  WHERE r.revision IS NULL OR json_extract(e.config_json, '$.communication.mode')
                            <> json_extract(r.config_json, '$.communication.mode')
);
INSERT INTO schema_migrations (version, applied_at)
  VALUES (17, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));
INSERT INTO migration_guard SELECT NOT EXISTS (SELECT 1 FROM pragma_foreign_key_check);
INSERT INTO migration_guard SELECT NOT EXISTS (
  SELECT 1 FROM pragma_integrity_check WHERE integrity_check <> 'ok'
);
COMMIT;
```

Before starting the new server, verify that the maximum version is 17, integrity returns `ok`, foreign-key checks return no rows, and the four row counts match the backup. Compare each revision's mode with the backup's old flag by experiment ID and revision.

## Pilot before opening intake

1. In a typed experiment, enter from two independent browser profiles, send one message from each, and finish the game. Each browser should show both messages once without requesting microphone access.
2. Use the same game package in a speech experiment. Check microphone permission, the meter, mute, and partner playback. For a spoken agent, also check recognition and audible replies with your configured providers.
3. Replay a recorded session, compare a representative export with the backup, and confirm that the role views, dialogue origins and completion agree. Run the [communication stress matrix](runtime-stress-testing.md) when validating a host or release candidate.
