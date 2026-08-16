# Multi-Experiment Game Server Implementation Plan

## Status

Proposed implementation plan, agreed on 2026-08-15.

## Objective

Change Parlando from one process owning one experiment to one process owning one
compiled game and any number of experiments for that game. The authenticated
dashboard becomes the source of truth for experiment and shared game settings;
experimenters no longer need to create or edit YAML files.

The design keeps the compilation boundary explicit:

- a process is compiled with exactly one `GameAdapter` implementation;
- every experiment managed by that process uses that compiled game;
- different games use different processes and different dashboards;
- there is no installation-wide dashboard spanning games; and
- there is no dynamic Rust plugin or umbrella-binary mechanism.

## Settled Design Decisions

### Process and dashboard identity

The running binary exposes a game descriptor containing a stable game id,
human-readable game name, and semantic version. The dashboard displays the game
name and version prominently and states that it manages experiments for this game
only. Experiment creation never asks the administrator to select a game.

### Exact game-version binding

Each experiment records the semantic game version of the process that created it.
An experiment can be activated only when its recorded version exactly equals the
running process's game version. There is no compatibility matrix, compatible-version
range, or automatic compatibility inference.

Experiments created under older versions remain available for browsing and export.
To run a related experiment under the current game version, the administrator clones
the old experiment. The clone receives a new experiment id, is bound to the current
game version, starts inactive, and must pass the current configuration validator.
The source experiment is never rewritten.

Build metadata such as the Git commit, dirty-worktree marker, client version, and
Parlando server version remains useful session provenance, but it does not
participate in the activation compatibility check.

### Two-state experiment lifecycle

Every experiment is either `inactive` or `active`. Creation and cloning always
produce inactive experiments. Activation opens new participant intake. Deactivation
closes new intake without terminating existing sessions.

Pinning, notes, and obsolescence are catalogue metadata rather than additional
lifecycle states. On process startup, every experiment is reset to inactive.

### Configuration ownership

Configuration is divided into three explicit layers:

1. **Bootstrap options** are needed before the database and dashboard are available.
   They remain CLI arguments or environment overrides.
2. **Game settings** apply to every experiment served by this compiled game process.
   They are edited in the dashboard and stored in the game database.
3. **Experiment settings** affect one experiment. They are edited in the dashboard,
   validated by the server, and stored as immutable revisions.

The institution belongs to game settings and is therefore shared by all experiments
served by the process. It is not part of an experiment revision.

YAML ceases to be the primary configuration source. It may remain as an import and
export format, but the database-backed typed configuration is authoritative. YAML
includes, relative-path merging, and environment substitution are not part of the
new stored experiment model.

### Ports and URLs

The port is a bootstrap argument because the dashboard cannot be reached until the
process has bound a socket, and several game processes may run on one machine. Keep
the existing `--host` support and make the port explicit for normal startup rather
than hiding process selection in experiment configuration.

Do not add `--external-url`. Each game process has a distinct origin through its
port. Browser code uses relative HTTP paths and derives WebSocket URLs from the
current page origin. Participant links are the current process origin plus the
experiment path, for example `/e/spatial-pilot/`.

The durable database location is also necessarily a bootstrap concern. Provide a
stable game-id-based default data location and retain an explicit `--database-url`
or `--data-dir` override for tests, backups, parallel development instances, and
operators who need a particular location. It is not editable per experiment.

### Secrets

Provider credentials, administrator recovery credentials, and signing material are
secrets rather than experiment settings. They must not enter experiment revisions,
exports, or browser responses. Continue resolving them through named environment or
mounted-file inputs. The dashboard may show whether a named credential is available
and allow experiments to select a non-secret credential name, but it must not return
the resolved value.

Adding browser-managed encrypted secret storage is a separate feature and is not a
prerequisite for removing experiment YAML.

## Target Architecture

### Compiled game descriptor

Add a typed descriptor supplied by the game binary alongside its adapter:

```rust
pub struct GameDescriptor {
    pub id: &'static str,
    pub display_name: &'static str,
    pub version: &'static str,
    pub build_manifest: serde_json::Value,
}
```

The descriptor is immutable process identity. Validate its id and semantic version
at startup. The Space Game binary supplies its descriptor when constructing the
server.

### Configuration types

Replace the monolithic `ExperimentConfig` boundary with the following conceptual
types:

```rust
pub struct BootstrapOptions {
    pub host: IpAddr,
    pub port: u16,
    pub database_url: String,
}

pub struct GameSettings {
    pub institution: String,
    pub credential_references: CredentialReferences,
    // Other settings shared by every experiment of this game.
}

pub struct ExperimentDefinition<G> {
    pub experiment_id: String,
    pub game_version: String,
    pub settings: ParlandoExperimentSettings,
    pub game: G,
}
```

Infrastructure fields currently nested under `server` and `database` move to
bootstrap options. Institution moves out of `StudyConfig`. Experiment-specific
study, consent, matchmaking, agent, voice, transcription, TTS selection, privacy,
and game-condition values remain in the experiment definition.

The exact boundary between shared provider settings and experiment provider choices
must be applied consistently: credentials and service endpoints are shared game
settings or bootstrap secrets; language, model, voice choice, and whether a feature
is enabled may vary by experiment.

### Game-specific experiment options

Extend the compiled-game integration with a concrete game-specific experiment
options type and validator. Avoid retaining an unstructured `serde_json::Value` as
the main configuration API.

The game integration must provide:

- default experiment options;
- a serializable schema for dashboard form construction;
- authoritative server-side deserialization and validation;
- construction of an adapter and agent factory from a validated experiment
  definition; and
- the game descriptor and client build manifest.

Keep the game runtime strongly typed. Because every experiment in the process uses
the same compiled game, no type erasure or heterogeneous adapter registry is needed.

### Process state

Refactor the current singleton experiment fields into an installation/game-server
state and per-experiment runtime state:

```rust
pub struct GameServerState<A: GameAdapter> {
    pub descriptor: GameDescriptor,
    pub game_settings: GameSettings,
    pub store: SharedExperimentStore,
    pub admin_auth: AdminAuthenticator,
    pub experiments: RwLock<HashMap<String, Arc<ExperimentRuntime<A>>>>,
}

pub struct ExperimentRuntime<A: GameAdapter> {
    pub experiment_id: String,
    pub game_version: String,
    pub config_revision: i64,
    pub config: ExperimentDefinition<...>,
    pub adapter: A,
    pub lifecycle: RwLock<ExperimentLifecycle>,
    // Existing room, agent, provider, credential, ticket, and connection state.
}
```

Move experiment-specific limits and registries into `ExperimentRuntime`. Keep
administrator authentication, database access, game identity, and shared settings
at `GameServerState` level.

Initially, it is acceptable to retain empty inactive runtime objects until process
restart. Do not add a background runtime-unloading system unless retained runtimes
become a measured problem. Deactivation still retains a runtime while existing
sessions finish.

### Routing

Use server-controlled experiment paths:

- `/admin` and `/api/admin/*` are game-server-wide;
- `/e/:experiment_id/` serves the participant client;
- `/e/:experiment_id/api/*` contains participant HTTP operations; and
- `/e/:experiment_id/ws/*` contains game and audio WebSockets.

Resolve the experiment from the route before authentication and never accept a
participant-supplied experiment id as authority in a request body. Participant
credentials, tickets, rooms, and rate limits remain scoped to the resolved runtime.

The client uses relative URLs rooted at its experiment path. Add one shared helper
for constructing experiment-relative HTTP and WebSocket endpoints so generated
clients do not duplicate path logic.

### Dashboard

The global header shows:

- Parlando;
- the compiled game's display name;
- the exact game version; and
- concise build details or a link to them.

The main page lists all experiments for this game, ordered by active status, pinning,
and recency. Each row shows version, lifecycle, session counts, last activity, and
obsolete state. Version-mismatched experiments are read-only and clearly marked as
belonging to an older game version.

Provide the following operations:

- create an inactive experiment from current defaults;
- clone any historical experiment into a new inactive current-version experiment;
- edit settings for an inactive current-version experiment;
- activate and deactivate intake;
- pin or unpin;
- mark or unmark obsolete;
- edit notes;
- inspect sessions and exports; and
- inspect and restore configuration revisions as a new revision.

An active experiment's effective configuration is immutable. The initial
implementation does not need pending revisions: the administrator deactivates the
experiment, waits for live sessions to finish, and then edits it. Pinning, notes, and
obsolete metadata remain editable independently.

### Form and schema contract

Generate dashboard forms from a server-provided schema derived from the typed
Parlando and game-specific settings. Add presentation metadata only where schemas
cannot express labels, ordering, units, help text, conditional fields, or multiline
content cleanly.

Browser validation improves feedback, but every save must deserialize into the
current Rust types and run semantic validation before a revision is committed.
Unknown fields continue to fail closed.

Use optimistic concurrency for saves. A request identifies the revision it edited;
the server rejects it if another administrator has already created a newer revision.

## Durable Data Model

Add or reshape the following storage concepts:

### Game settings

A singleton game-settings row stores the institution and other non-secret settings
shared by the process. Store its own revision or `updated_at` value so changes are
auditable and concurrent updates can be rejected.

### Experiments

The experiment catalogue row contains:

- experiment id;
- exact game version;
- current configuration revision id;
- `active`/`inactive` status;
- pinned flag or sort priority;
- obsolete flag;
- notes;
- creation and update timestamps; and
- compact session aggregates where useful.

### Experiment configuration revisions

Store immutable revisions containing:

- experiment id and monotonically increasing revision;
- normalized secret-free configuration JSON;
- creation timestamp;
- administrator identity when available; and
- optional change summary.

Do not overwrite old revision JSON. Session creation records the current revision.

### Session provenance

Every new session records at least:

- experiment id;
- experiment configuration revision;
- exact game version; and
- existing server, game, and client build manifest information.

Exports expose this provenance without exposing secrets.

### Historical rows

Existing databases contain a persisted configuration snapshot in each experiment
row. Migrate that snapshot into revision 1, copy the recorded game version from its
version manifest when available, and otherwise mark it with an explicit legacy or
unknown value that cannot be activated accidentally.

The migration preserves sessions and exports. Do not keep compatibility routes or
duplicate legacy configuration code after the migration is established and tested.

## Administrative API Shape

Introduce game-server-wide endpoints along these lines:

```text
GET    /api/admin/game
GET    /api/admin/game/settings
PUT    /api/admin/game/settings

GET    /api/admin/experiments
POST   /api/admin/experiments
GET    /api/admin/experiments/:id
POST   /api/admin/experiments/:id/clone
PUT    /api/admin/experiments/:id/config
POST   /api/admin/experiments/:id/status
PATCH  /api/admin/experiments/:id/catalogue
GET    /api/admin/experiments/:id/revisions
```

Session, event, deletion, and export routes become explicitly experiment-scoped.
Retain the existing authenticated administrator, role, and CSRF boundaries for every
new operation. Experiment mutation requires administrator capability; inspection and
export may retain operator capability.

## Implementation Phases

### Phase 1: Define boundaries without changing behavior

1. Add `GameDescriptor` and require the Space Game server to provide id, display
   name, semantic version, and build manifest.
2. Split bootstrap-only fields out of the conceptual experiment configuration while
   retaining temporary loader conversion at the outer boundary.
3. Introduce typed shared `GameSettings` and typed game-specific experiment options.
4. Add focused validation and serialization tests for all new types.
5. Update generated-game guidance so every game declares its descriptor and
   experiment-options schema.

Acceptance criteria:

- current single-experiment configurations still start through a temporary migration
  adapter;
- the dashboard identifies the compiled game and version; and
- no runtime behavior changes yet.

### Phase 2: Add catalogue and configuration revisions

1. Add schema migrations for game settings, exact experiment game version,
   catalogue metadata, configuration revisions, and session provenance.
2. Restore storage operations for listing, creating, cloning, and reading multiple
   experiments.
3. Make creation use current typed defaults and the running game version.
4. Make cloning copy configuration into a new id bound to the running version and
   validate it under current types.
5. Migrate existing persisted configurations to revision 1.
6. Add exact-version activation checks.

Acceptance criteria:

- historical experiments remain browseable;
- version-mismatched experiments cannot activate;
- cloning never changes the source experiment; and
- every new session records its config revision and game version.

### Phase 3: Refactor the runtime to multiple experiments

1. Extract `ExperimentRuntime<A>` from the current `AppState<A>`.
2. Add `GameServerState<A>` with the shared store, admin authentication, descriptor,
   shared settings, and runtime map.
3. Move rooms, waiting state, lifecycle, participant authentication, tickets, agent
   state, provider selection, and live connections into the resolved runtime.
4. Make creation and activation construct a runtime from a stored validated revision.
5. Reset every experiment inactive during startup.
6. Preserve the current rule that deactivation leaves existing sessions connected.

Acceptance criteria:

- two experiments of the same compiled game can be active simultaneously;
- their waiting rooms, participants, agents, credentials, and status are isolated;
- deactivating one does not affect the other; and
- restarting the process makes both inactive.

### Phase 4: Introduce experiment-scoped participant routing

1. Move participant HTTP and WebSocket routes under `/e/:experiment_id`.
2. Serve the compiled client at each experiment root.
3. Update the JavaScript client to use experiment-relative endpoints.
4. Derive copyable participant links from the current browser origin and experiment
   path; do not introduce external URL configuration.
5. Remove old unscoped participant routes after coordinated client migration.

Acceptance criteria:

- direct links always enter the selected experiment;
- request bodies cannot change experiment scope;
- WebSocket and audio tickets cannot cross experiments; and
- multiple local game processes remain distinguishable solely by their ports.

### Phase 5: Build database-backed dashboard editing

1. Add game-settings UI with institution editing.
2. Add experiment creation, cloning, catalogue, and lifecycle controls.
3. Expose configuration schemas and render all experimenter-controlled options as
   structured form fields.
4. Add authoritative save validation, optimistic concurrency, revision history, and
   revision diffs.
5. Prevent effective-configuration edits while active or while sessions remain live.
6. Show version mismatches and validation failures as actionable dashboard states.

Acceptance criteria:

- a fresh process can create, configure, activate, inspect, and export an experiment
  without an experiment YAML file;
- every successful save creates an immutable revision;
- invalid or stale edits never replace the effective revision; and
- the game name, game version, and shared institution are unambiguous throughout the
  dashboard.

### Phase 6: Remove YAML as a startup dependency

1. Change game binaries to start from bootstrap CLI/environment options and database
   state rather than `ExperimentConfig::from_yaml`.
2. Make `--port` explicit in documented normal startup; retain `PORT` only where a
   hosting platform requires it.
3. Provide a stable default data location plus an explicit override.
4. Retain YAML only as a secret-free import/export representation if it remains
   useful.
5. Delete include merging, relative experiment-config paths, and environment
   substitution from the ordinary runtime path once migrations and imports no longer
   depend on them.
6. Update Docker, Render, local Make targets, README, deployment documentation, and
   game-generation templates.

Acceptance criteria:

- first startup needs no experiment configuration file;
- the server can start with zero experiments;
- the setup flow creates the administrator and first experiment through the browser;
  and
- two game binaries can run locally using different explicit ports and data stores.

### Phase 7: Verification and release cleanup

1. Add integration tests with two simultaneous experiments covering activation,
   matchmaking, agent games, audio tickets, session persistence, exports, deletion,
   and restart behavior.
2. Add migration fixtures for current databases and version-mismatched historical
   experiments.
3. Test concurrent configuration edits and cloning under the current game version.
4. Verify that configuration APIs, revision history, YAML export, logs, and build
   manifests contain no resolved secrets.
5. Remove singular experiment names, compatibility aliases, and transitional loader
   code.
6. Update the changelog and technical documentation to describe one compiled game per
   process and many experiments per dashboard.

## Testing Matrix

At minimum, automated tests must cover:

| Area | Required cases |
| --- | --- |
| Game identity | Valid semantic version; dashboard identity; exact match and mismatch |
| Catalogue | Create; clone; pin; obsolete; notes; ordering |
| Configuration | Defaults; all form fields; strict validation; revision history; stale save |
| Lifecycle | Create inactive; activate; deactivate; restart inactive |
| Isolation | Two active experiments; separate queues, rooms, credentials, agents, and exports |
| Routing | Correct path scope; missing/unknown experiment; cross-experiment credential rejection |
| Versioning | Historical browsing; mismatch rejection; clone binds current version |
| Provenance | Session records exact game version, config revision, and build manifest |
| Security | Admin roles; CSRF; secret-free persistence and responses |
| Migration | Existing singleton database becomes a valid one-experiment game database |

## Documentation Deliverables

Update the following contracts as implementation lands:

- researcher workflow and first-start setup;
- game-author API for descriptor and typed experiment options;
- dashboard configuration and revision semantics;
- exact-version activation and clone-to-upgrade workflow;
- participant URL format;
- CLI bootstrap arguments and multi-game local port examples;
- database backup and migration behavior; and
- generated-game templates and deployment examples.

## Explicit Non-Goals

- No dashboard spanning multiple compiled games.
- No process supervisor or automatic child-process cleanup.
- No dynamically loaded Rust game plugins.
- No umbrella binary linking unrelated games.
- No game-version compatibility matrix or range resolution.
- No reactivation of an old-version experiment under a new-version process.
- No external URL configuration.
- No hot mutation of the effective configuration of an active experiment.
- No browser-managed encrypted secret store in this project phase.
- No automatic unloading of inactive experiment runtimes unless measurements justify
  it.

## Principal Risks

1. The current application module is heavily parameterized around one singleton
   `AppState<A>`. Extract runtime ownership before adding route conditionals so the
   refactor yields a clean boundary rather than scattered experiment-id lookups.
2. The existing configuration combines deployment, secret, shared-game, and
   experiment concerns. Persisting that structure unchanged would recreate YAML's
   coupling inside SQLite; perform the type split early.
3. Generated schema forms can become difficult to use if schema presentation metadata
   is an afterthought. Treat labels, explanations, ordering, units, and conditional
   sections as part of the game-author contract.
4. Current clients assume process-root API and WebSocket paths. Coordinate server and
   client route changes and remove the old paths after migration rather than retaining
   a permanent dual contract.
5. Historical configuration JSON may not deserialize under future game versions.
   Keep historical browsing storage-oriented and require current validation only for
   cloning and activation.

