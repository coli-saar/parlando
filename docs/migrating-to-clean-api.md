# Migrating games to the clean Parlando API

This is the authoritative migration specification for games written against Parlando 0.2. It is intentionally explicit enough for an LLM to execute. The target API has no compatibility aliases: port callers and delete obsolete code.

The target release uses `parlando = "0.3.0"` and `@coli-saar/parlando-client` `^0.3.0`. For dependency replacement, migration sequencing, repository-wide auditing, and verification commands, start with [Migrate a Parlando Game from 0.2 to 0.3](migrating-0.2-to-0.3.md), then return here for the complete API mapping.

## Invariants to preserve

- The Rust runtime alone owns authoritative game state.
- Players and agents receive the same accepted actions and shared `Completion`, together with the `Observation` projected for their own `PlayerRole`.
- There are exactly two roles, `PlayerRole::A` and `PlayerRole::B`; either can be human or automated.
- Actions are the only player operations that can change state.
- Messages travel only between players and cannot change state by themselves.
- Games and agents exchange structured domain data, not human-rendered presentation.
- The Rust server exposes a versioned, frontend-neutral protocol. The JavaScript client and React components are optional conveniences.

## Migration order

1. Port the Rust `Game` implementation.
2. Port Rust or Python agents.
3. Replace server construction.
4. Port the participant application and any custom protocol client.
5. Delete old API code and run the checklist.

## Rust game

| Old | New |
|---|---|
| `GameAdapter` | `Game` |
| `GameDescriptor` | `GameMetadata` |
| `GameDescriptor.display_name` | `GameMetadata.name` |
| `type Event` | Delete |
| `type Summary` | `type Completion`; derive or implement `Clone` as well as `Serialize` |
| `initial_state()` and `initial_state_with_config(config)` | `initial_state(config, seed) -> anyhow::Result<State>` |
| `parse_action(Value)` | Delete; implement `Deserialize` for `Action` |
| `validate_action(...)` then `apply_action(...)` | `apply_action(state, action, actor) -> Result<State, ActionRejection>` |
| `observe_state(state, player)` | `observation(state, role)` |
| `events_for_action(...)` | Delete, or use `transition_metadata(...)` for analysis data |
| `is_complete(...)` and `completion_summary(...)` | `completion(state) -> Option<Completion>` |

Target shape:

```rust
use parlando::{ActionRejection, Game, PlayerRole};

impl Game for MyGame {
    type Config = MyConfig;
    type State = MyState;
    type Action = MyAction;
    type Observation = MyObservation;
    type Completion = MyCompletion;

    fn validate_config(&self, config: &Self::Config) -> anyhow::Result<()> {
        validate_config(config)
    }

    fn initial_state(&self, context: GameInitializationContext<'_, Self::Config>) -> anyhow::Result<Self::State> {
        create_initial_state(context.config, context.seed)
    }

    fn apply_action(
        &self,
        state: &Self::State,
        action: &Self::Action,
        actor: PlayerRole,
    ) -> Result<Self::State, ActionRejection> {
        apply_checked_action(state, action, actor)
    }

    fn observation(&self, state: &Self::State, role: PlayerRole) -> Self::Observation {
        observe(state, role)
    }

    fn available_actions(
        &self,
        state: &Self::State,
        role: PlayerRole,
    ) -> Option<Vec<Self::Action>> {
        available_actions(state, role)
    }

    fn transition_metadata(
        &self,
        before: &Self::State,
        after: &Self::State,
        action: &Self::Action,
        actor: PlayerRole,
    ) -> Option<serde_json::Value> {
        transition_metadata(before, after, action, actor)
    }

    fn completion(&self, state: &Self::State) -> Option<Self::Completion> {
        completion(state)
    }
}
```

`apply_action` must perform role authorization and state-dependent validation atomically. Return `ActionRejection::new("stable_code")` for an expected rule rejection. The code must not contain localized prose, secrets, or diagnostics.

`Observation` is the complete information available to one role. Remove all fallbacks to serialized `State`. If a participant application needs another domain fact, add it to `Observation` without exposing private state.

Participant transition messages contain the actor, accepted `Action`, recipient-specific resulting observation, and nullable action affordances. This matches Rust and remote agents, which receive the same transition information through `observe_transition`. A frontend may ignore the action and render only the observation. Accepted actions are observable to both roles; represent a deliberately hidden cause with a non-revealing action value such as `SecretAction`. `Completion` remains the shared, game-specific terminal payload for facts such as win/loss, winner, and scores. It must contain only facts safe for both roles; put private terminal facts in the final observation. Do not replace it with a generic Parlando outcome schema.

`available_actions` is an optional affordance. `None` means the game does not enumerate its action space; `Some(vec![])` means it does and none are available. `apply_action` remains authoritative.

Delete participant events. Derive presentation from observations or accepted transitions. Move only role-neutral, structured analysis data needed by logs or the dashboard to `transition_metadata`; never return viewer-specific prose there.

## Rust agents

| Old | New |
|---|---|
| `GameAgent<G>` | `agent::Agent<G>` |
| `AgentFactory<G>` | `agent::Factory<G>` |
| `AgentInitContext` | `agent::Context` |
| `AgentParticipantIdentity` | `agent::Identity` |
| `AgentResponse<A>` | `agent::Response<A>` |
| `AgentFactoryDescriptor` | `agent::Definition` |
| `AgentConfigFieldDescriptor` | `agent::ConfigField` |
| `RemoteGrpcAgentFactory` | `agent::grpc::Factory` |
| `observe_state(observation)` | `start(initial_observation)` |
| `observe_action(actor, action, observation)` | `observe_transition(actor, action, observation)` |
| `observe_message(speaker, kind, text)` | `observe_message(sender, text)` |
| No terminal-result callback | `finish(completion)` |
| `maybe_act(...)` or `act(...)` | `respond(...)` |
| Optional response fields | Non-empty `Response` enum variants |

`Factory::create(context).await` receives only `role`, `seed`, and agent-owned `settings`. It constructs and initializes the agent before a game state exists. After creation succeeds, the runtime creates the initial state and calls `Agent::start(initial_observation)`. Do not require an observation in the constructor.

Every factory must implement `identity(settings)` and return an `agent::Identity` with non-empty `name` and `version` strings. The version identifies the implementation release. Keep model, prompt, endpoint, and other configuration choices in the normalized settings; Parlando incorporates those separately into the configuration fingerprint. Newly created automated participants cannot be unversioned, although historical stored rows without version metadata remain readable.

Return `Response::action(action)`, `Response::message(text)`, or `Response::action_and_message(action, text)`. Return `Ok(None)` when declining to respond. A message goes only to the other player. In a combined response, the runtime accepts the action before sending the message.

Register factories on `Server`, not on the game:

```rust
let server = Server::new(game, metadata)?
    .agent(MyFactory::new())?
    .agent(agent::grpc::Factory::<MyGame>::new())?;
```

## Python remote agents

| Old | New |
|---|---|
| `GameAgent` | `Agent` |
| `AgentResponse` | `Response` |
| `serve_agent(...)` | `serve(...)` |
| `observe_state(...)` | `start(initial_observation)` |
| `observe_action(...)` | `observe_transition(...)` |
| `observe_message(speaker, kind, text)` | `observe_message(sender, text)` |
| No terminal-result callback | `finish(completion)` |
| `maybe_act(...)` | `respond(...)` |
| `AgentResponse.say(text)` | `Response.message(text)` |

```python
from parlando_agent_sdk import Agent, Context, Response, serve

class MyAgent(Agent):
    async def start(self, initial_observation):
        self.observation = initial_observation

    async def observe_transition(self, actor, action, observation):
        self.observation = observation

    async def observe_message(self, sender, text):
        self.last_message = (sender, text)

    async def finish(self, completion):
        self.completion = completion

    async def respond(self, available_actions):
        if available_actions:
            return Response.action(available_actions[0])
        return None

def create_agent(context: Context) -> Agent:
    # Load models and other agent-owned resources here. The initial observation
    # is delivered only after this function returns.
    return MyAgent()

serve(create_agent, host="127.0.0.1", port=50051)
```

`Context` contains only `role`, deterministic `seed`, and agent-owned `settings`. It contains no room identity, participant identity, frontend data, transport data, or initial observation. Regenerate protobuf bindings from the proto shipped with the installed SDK. Do not use generated RPC DTOs as the agent-authoring API.

## Server construction

Delete imports of `ExperimentConfig`, `ServeOptions`, `serve`, `serve_game`, routers, provider configuration, storage types, and generated agent protobufs. Replace them with:

```rust
use parlando::{GameMetadata, Server};

Server::new(
    MyGame,
    GameMetadata {
        id: "my-game".into(),
        name: "My Game".into(),
        version: env!("CARGO_PKG_VERSION").parse()?,
        build_manifest: serde_json::json!({}),
    },
)?
.database_url("sqlite:///./parlando.sqlite")
.participant_app("../client/dist")
.serve("127.0.0.1:3000".parse()?)
.await?;
```

Do not reconstruct dashboard-owned settings in user code. Participant information and consent copy, pairing, capacity, session limits, privacy, voice, transcription, TTS, and selected agent settings are experiment configuration. The researcher chooses the unique experiment ID when creating an experiment; the dashboard treats that ID as immutable afterward. There is no separate experiment-name or participant-title setting. Participant startup identifies the compiled game by `GameMetadata.name`.

When transferring an old checked-in or stored YAML configuration into the dashboard, move only the lifecycle fields from the `study` object to `session` and discard `study.name`:

```yaml
# Old
study:
  name: My Study
  waiting_room_timeout_seconds: 600

# New
session:
  waiting_room_timeout_seconds: 600
```

Do not turn a legacy study into a session record, and do not rename the removed `name` to `experiment_name`, `participant_title`, or another presentation setting. A research study remains an external organizing concept; Parlando models game, experiment, and session.

## JavaScript and React

Supported entry points:

```ts
import { ParticipantClient } from "@coli-saar/parlando-client";
import {
  ParticipantApp,
  type GameSession,
  MicrophoneLevelMeter,
  MicrophoneMuteButton,
  TranscriptionProgress,
} from "@coli-saar/parlando-client/react";
```

| Old | New |
|---|---|
| `ExperimentApiClient` | `ParticipantClient` |
| `ParlandoStartupGate` | `ParticipantApp` |
| `ParlandoStartupGateProps` | `ParticipantAppProps` |
| `ActiveParlandoSession` | `GameSession` |
| `session.completionSummary` | `session.completion` |
| `session.sendChatMessage(text)` | `session.sendMessage(text)` |
| `session.state` | Delete |
| `session.events` and event generic | Delete |
| `session.participantSessionId` | Delete |
| `session.publicConfig` | Delete; use narrow session capabilities |
| `ConversationMessage` | `PlayerMessage` |
| message `origin` | message `input`, either `"text"` or `"voice_transcript"` |
| `availableActions?: Action[]` | `availableActions: Action[] \| null` |
| transition `action` | Retain; it is shared player information alongside the role-specific observation |
| `MicrophoneControl` | `MicrophoneMuteButton` |
| `AudioLevelMeter` | `MicrophoneLevelMeter` |
| `TranscriptionStatus` | `TranscriptionProgress` |

Render from `session.observation`. Use transition data only for local animation or history, never as a second state channel. Submit typed actions with `session.sendAction(action)` and player messages with `session.sendMessage(text)`.

Construct the lower-level managed client with an options object: `new ParticipantClient({ baseUrl })`. `getExperiment()` returns camel-cased `ExperimentInfo` fields, including `status`, `participantInformationVersion`, and `participantInformationUrl`. `register()` retains the participant credential internally and returns `Promise<void>`; `join()` returns camel-cased `JoinedRoom` data without an internal participant-session identifier. `getGameSession()` and `getAudioSession()` likewise return the exported, camel-cased `GameSessionPlan` and `AudioSessionPlan` values. Other raw HTTP DTOs, server-message unions, credentials, socket helpers, diagnostics, and audio implementation classes are no longer package exports.

Direct protocol clients must require `protocol_version: 1` and map:

| Old wire type | New wire type |
|---|---|
| `roleAssigned` | `session_started` |
| `stateChanged` | `transition` |
| `conversationMessageAdded` | `message` |
| `presenceChanged` | `presence` |
| `voiceStatusChanged` | `voice_status` |
| `partnerWaiting` / `waiting` | Delete; derive waiting from join state and `presence` |
| client `submitAction` | client `action` |
| client `sendChatMessage` | client `message` |

The protocol sends observations, accepted actions, nullable available actions, individual player messages, presence, voice status, and shared completion. `session_started` and `transition` no longer include participant-session IDs, conversation snapshots, completion placeholders, or extra presentation fields. A `message` contains `message`, not `conversation_message`; `completed` contains `completion`, not `summary`; errors contain stable `code` and `fatal`, not presentation prose. It never sends authoritative state or generic game events. The only accepted client messages are `ready`, `action`, `message`, `heartbeat`, and `leave`; consent uses HTTP. Reject unknown protocol versions.

## Removed public implementation APIs

Do not replace these old imports with new public equivalents:

- dashboard/runtime configuration such as `ExperimentConfig`, pairing mode, session limits, provider configuration, privacy configuration, and capacity configuration;
- storage records, application state, routers, handlers, generated protobuf modules, and provider traits;
- JavaScript protocol DTOs, raw socket helpers, audio controllers, microphone sources, audio sinks, and startup test seams; and
- Rust `test_support` in ordinary builds.

Parlando's own tools compile the last category behind the unsupported `internal-tools` feature. Downstream games must test through the public `Game`, `Server`, agent, participant-client, and wire-protocol contracts instead.

## Deletion pass

Search the migrated project and delete every remaining old authoring symbol:

```text
GameAdapter GameDescriptor GameAgent AgentResponse AgentInitContext
ServeOptions ExperimentConfig serve_game events_for_action observe_state
observe_action maybe_act ParlandoStartupGate ActiveParlandoSession
ExperimentApiClient completionSummary sendChatMessage roleAssigned stateChanged
```

Do not add deprecated aliases. Port or delete tests that depend on obsolete behavior.

## Verification checklist

- The game crate does not import `test_support` or internal Parlando modules.
- No participant title or `study` lifecycle block remains.
- Identical configuration, seed, and accepted actions produce identical results.
- Observation tests prove that neither role receives the other's private data.
- Invalid role/action combinations return stable rejection codes.
- Messages alone leave state unchanged.
- Agents are constructed before `start` receives an observation.
- Agents receive the same accepted actions and shared completion as human players, with role-safe observations and messages from the other player.
- The browser never reads serialized authoritative state.
- Custom clients reject unsupported protocol versions.
- Rust, JavaScript, Python, and end-to-end suites pass.

## Deferred readiness question

This migration preserves the current rule: an agent is considered constructed when `Factory::create(...).await` (or remote `CreateAgent`) returns; the current runtime temporarily bounds that await with the selected action timeout, and `start(initial_observation)` then begins game delivery. It does not define a separate readiness signal, initialization timeout/cancellation semantics, or isolation for synchronous model loading. Review these as the first follow-up after this API change; do not invent game-specific readiness messages while migrating.
