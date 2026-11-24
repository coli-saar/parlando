# Migrate from Parlando 0.6.0 to 0.7.0

Upgrade the server, game tooling and browser assets together. Version 0.7.0 runs games as WebAssembly components and makes the participant communication channel an explicit experiment setting. Keep the 0.6.0 executable and a backup of its database together for replay of native sessions. Converting the database updates experiment configuration. It cannot reconstruct a native session with a newly compiled Wasm game.

## Build the game and browser client

Use the 0.7.0 versions of `parlando` and `@coli-saar/parlando-client`. Use matching versions of the Rust game SDK and derive crate, or the Python game and agent SDKs, wherever your project depends on them. For an unreleased candidate, use the candidate artifacts. After publication, install the coordinated versions from their registries.

For a custom Rust host, update the dependency in `Cargo.toml`:

```toml
[dependencies]
parlando = "0.7.0"
```

A host that previously supplied a native `GameFactory` now loads the compiled game component with `LoadedGame`. In a TypeScript participant client, update the existing dependency in `package.json` and regenerate its lockfile:

```json
{
  "dependencies": {
    "@coli-saar/parlando-client": "^0.7.0"
  }
}
```

Start a reference project with `parlando new <game-id> --lang rust` or `--lang python`. Move game mechanics into the session-owned component. For Rust games the public `Game` trait lives in `parlando_game_sdk`. Projects using the earlier component SDK should rename `parlando-component-sdk` to `parlando-game-sdk` and replace `ComponentWire` and `ComponentDecode` with the `GameValue` derive. Build for the v4 game world. Derive each role’s complete observation from the private state, including on the terminal turn. Put any activity history that must survive a browser reload in that role’s view. See the [component interface](component-games.md) and [agent integration guide](agents.md).

Run `parlando test` and `parlando build`. Pass the ZIP path printed by the build to `parlando check --package`. Deploy the resulting ZIP to your host. Retain the exact component used to record each session you wish to replay. See [package a game](web/content/manual/build/packages.md).

### Verify the browser package used by a candidate build

A game version in `game.yaml` identifies the game ZIP. Its browser SDK comes from the client dependency and lockfile. A locally built candidate can share its npm version number with an older published SDK. Install the candidate tarball into the game’s client before building. From the game root, run the following with the actual tarball path.

```sh
npm --prefix client install /absolute/path/to/coli-saar-parlando-client-VERSION.tgz
parlando build
```

Keep the tarball at the path recorded in the lockfile for reproducible builds. The CLI checks that an SDK-backed browser project can decode the server’s typed and speech configurations after npm installs its dependencies. For prebuilt browser distributions, pilot the actual ZIP in the intended experiment mode.

In a speech experiment with a software partner, prepare the microphone and verify the selected device name and a moving level meter. Enter the waiting room. When transcription gates startup, the human’s audio connection must reach provider readiness and the agent must finish initialization. Both seats can already show a connection during this interval. Reloading the waiting room requires preparing the browser microphone again. For Beacon, select `beacon.tick_on_request`, take the opening turn, and say “charge the beacon.” Verify the agent adds one tick after the finalized transcript and returns the turn. A speech experiment that allows entry without preparing voice and shows no voice panel can indicate a client that still reads `voice.enabled`. Rebuild and deploy a new game revision using the matching SDK.

## Update a software partner

For a packaged Rust policy, import `Agent`, `Observation`, `RespondReason` and `Response` from `parlando_game_sdk::agent`. Export the policy with `parlando_game_sdk::export_agent!` and build it separately from the game. A standalone policy can generate its action, view and completion types from the compiled game, even when the game is written in Python.

Run `parlando agent new <agent-id> --game "$PACKAGE"` to start a policy against the game ZIP. For an existing policy, run the following from the policy project with `PACKAGE` set to the new game ZIP’s absolute path.

```sh
parlando agent set-game "$PACKAGE" --manifest agent.yaml
parlando build --manifest agent.yaml
```

Selecting the ZIP records its version and digest and refreshes any generated bindings. Update policy code that uses changed types, then check the printed agent ZIP with `parlando check --package`. Deploy the game first, upload the agent ZIP, and choose its factory in the inactive experiment’s configuration. For headless experiments with a custom Rust reward function, move parameter handling into `RewardFunction::configure`, which runs once per run. See the [agent guide](agents.md).

## Deploy the game

Start the host with `parlando serve --store DIRECTORY`. Open `/` on the printed administrator origin and sign in. For CLI uploads, run `parlando login --server "$HOST"`, choose **Authorize CLI** in the browser, then run `parlando deploy --host "$HOST" --package "$PACKAGE"`. You can also upload the ZIP from **Games**.

Deployment mounts the game immediately. Click the game ID, configure an experiment, and start **Local preview**. Each experiment controls its own intake. Before replacing an existing game, stop its experiments and let active sessions finish. A replacement under the same game ID keeps the research database, experiment URLs and installed agents. The host checks registered agents against the new payload layouts. Pilot the existing experiment and correct any incompatible game settings before opening official intake. Record the returned revisions and retain the ZIPs with the study data. See the [shared deployment guide](web/content/manual/deploy/_index.md).

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

## Validate the upgrade before opening intake

1. In a typed experiment, enter from two independent browser profiles, send one message from each, and finish the game. Each browser should show both messages once without requesting microphone access.
2. Use the same game package in a speech experiment. Check microphone permission, the meter, mute, and partner playback. For a spoken agent, also check recognition and audible replies with your configured providers.
3. If using Prolific, check a direct admission and a Prolific admission. Gate custom recruitment instructions on `snapshot.recruitmentSource === "prolific"` and let the SDK display the server-issued return handoff. A direct participant who exits and reloads should see neutral text.
4. Replay a newly recorded component session using its exact game build. For a historical native session, replay with the preserved 0.6.0 installation. Compare a representative export with the backup, and confirm that the role views, dialogue origins and completion agree. Run the [communication stress matrix](runtime-stress-testing.md) when validating a host or release candidate.
