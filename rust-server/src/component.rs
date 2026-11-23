//! Loads a versioned WebAssembly game component into either Parlando runner.
//!
//! Each session has one isolated instance with no host imports. A successful
//! step returns role-private views and can optionally include a recovery checkpoint.
//! The host loads the guest's wire schema before creating any sessions.
//! Reads use the host's explicitly selected frame, including during SQLite writes.

use std::{
    path::Path,
    sync::{Arc, Mutex},
};

use anyhow::{anyhow, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use wasmtime::{
    component::{Component, InstancePre, Linker},
    Config, Engine, Store, StoreLimits, StoreLimitsBuilder,
};

use crate::{ActionRejection, PlayerRole};

pub use parlando_game_sdk::{
    ComponentValue, GameSchema, ProjectedValue, Projection, Schema, Selection,
};

mod bindings {
    wasmtime::component::bindgen!({ path: "wit", world: "game" });
}

use bindings::exports::parlando::game::mechanics::{Frame, Role};

const MEMORY_LIMIT: usize = 64 * 1024 * 1024;
const CALL_FUEL: u64 = 50_000_000;

/// Execution budget selected by the host before compiling a game component.
/// The mode applies to schema discovery, initialization, turns and recovery.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum WasmMetering {
    /// Instrument each guest call with a fresh 50-million-unit deterministic budget.
    #[default]
    Fuel,
    /// Compile without fuel checks. A looping guest can occupy its worker indefinitely.
    None,
}

impl WasmMetering {
    /// Preserves the historical serialized experiment specification for the default.
    pub(crate) fn is_fuel(&self) -> bool {
        *self == Self::Fuel
    }

    /// Supply the initial or next call budget only to stores compiled with fuel support.
    pub(crate) fn begin_call(self, store: &mut Store<StoreLimits>) -> Result<()> {
        if self.is_fuel() {
            store.set_fuel(CALL_FUEL)?;
        }
        Ok(())
    }
}

/// Precompiled, import-free game implementing the `parlando:game@4.0.0` WIT world.
/// Clone this handle to share compilation across otherwise independent sessions.
#[derive(Clone)]
pub struct LoadedGame {
    bytes: Arc<[u8]>,
    metering: WasmMetering,
    compiled: Arc<InstancePre<StoreLimits>>,
    digest: String,
    schema: Arc<GameSchema>,
}

impl LoadedGame {
    /// Compiles a local component, verifies its interface and pins the SHA-256 of
    /// its bytes. Loads and checks the wire schema once using a separate guest
    /// with the same fuel and memory limits as session instances.
    pub fn load(path: &Path) -> Result<Self> {
        Self::load_with_metering(path, WasmMetering::Fuel)
    }

    /// Loads the file under the selected execution budget, including its schema call.
    pub fn load_with_metering(path: &Path, metering: WasmMetering) -> Result<Self> {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        Self::compile(bytes.into(), metering)
            .with_context(|| format!("loading game from {}", path.display()))
    }

    /// Compiles in-memory package bytes using the default fuel budget.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        Self::from_bytes_with_metering(bytes, WasmMetering::Fuel)
    }

    /// Compiles in-memory package bytes using the selected execution budget.
    pub(crate) fn from_bytes_with_metering(bytes: &[u8], metering: WasmMetering) -> Result<Self> {
        Self::compile(bytes.into(), metering)
    }

    /// Returns the instrumentation configured for this game.
    pub(crate) fn metering(&self) -> WasmMetering {
        self.metering
    }

    /// Changes instrumentation using the pinned bytes and preserves the checked schema.
    pub(crate) fn with_metering(mut self, metering: WasmMetering) -> Result<Self> {
        if self.metering != metering {
            self.compiled = Arc::new(Self::compile_component(&self.bytes, metering)?);
            self.metering = metering;
        }
        Ok(self)
    }

    /// Compiles the pinned game bytes with the selected store instrumentation.
    fn compile_component(bytes: &[u8], metering: WasmMetering) -> Result<InstancePre<StoreLimits>> {
        let mut config = Config::new();
        config.consume_fuel(metering.is_fuel());
        let engine = Engine::new(&config)?;
        let component = Component::from_binary(&engine, bytes)?;
        Ok(Linker::new(&engine).instantiate_pre(&component)?)
    }

    /// Compiles the original bytes and validates the initial description.
    fn compile(bytes: Arc<[u8]>, metering: WasmMetering) -> Result<Self> {
        let compiled = Self::compile_component(&bytes, metering)?;
        let mut guest = Guest::instantiate(&compiled, metering)?;
        guest.metering.begin_call(&mut guest.store)?;
        let description = guest
            .game
            .parlando_game_mechanics()
            .call_describe(&mut guest.store)
            .context("reading component schema")?;
        let schema = GameSchema::from_description(description.as_bytes())
            .context("validating component schema")?;
        Ok(Self {
            compiled: Arc::new(compiled),
            digest: format!("{:x}", Sha256::digest(&bytes)),
            bytes,
            metering,
            schema: Arc::new(schema),
        })
    }

    /// Identifies the exact executable loaded by this handle for provenance.
    pub fn digest(&self) -> &str {
        &self.digest
    }

    /// Returns the checked schema shared by all sessions using this game. A local
    /// policy can use it to exchange compact values directly with those sessions.
    pub fn schema(&self) -> &GameSchema {
        &self.schema
    }

    /// Checks that the configured value conforms to the component's declared input schema.
    pub fn validate_config(&self, config: &ComponentValue) -> Result<()> {
        config
            .to_schema_messagepack(&self.schema.config)
            .context("encoding component configuration")?;
        Ok(())
    }

    /// Creates the session's execution cache. Initialization or recovery supplies
    /// the first actual state before the first action.
    pub fn session(&self) -> GameSession {
        GameSession {
            loaded_game: self.clone(),
            current: Mutex::new(None),
        }
    }
}

/// Last selected game frame. The recovery recipe can rebuild
/// the guest, while the role views are safe to read without executing guest code.
#[derive(Clone, Serialize, Deserialize)]
pub struct GameFrame {
    component: String,
    recovery: Recovery,
    views: [ComponentValue; 2],
    completion: Option<ComponentValue>,
    metadata: Option<ComponentValue>,
    // A deserialized checkpoint must restore into a fresh guest. Clones retain
    // identity so normal turns never compare or hash potentially large states.
    #[serde(skip)]
    identity: Arc<()>,
}

impl GameFrame {
    /// Keeps codec-produced views, completion, and metadata in shared MessagePack buffers.
    fn from_frame(
        frame: Frame,
        loaded_game: &LoadedGame,
        recovery: impl FnOnce() -> Recovery,
    ) -> Result<Self> {
        let schema = loaded_game.schema();
        Ok(Self {
            component: loaded_game.digest().to_owned(),
            recovery: frame
                .checkpoint
                .map(Recovery::checkpoint)
                .unwrap_or_else(recovery),
            views: [
                ComponentValue::from_schema_bytes(frame.view_a, schema.view_a.clone())
                    .context("component role A observation")?,
                ComponentValue::from_schema_bytes(frame.view_b, schema.view_b.clone())
                    .context("component role B observation")?,
            ],
            completion: frame
                .completion
                .map(|bytes| ComponentValue::from_schema_bytes(bytes, schema.completion.clone()))
                .transpose()
                .context("component completion")?,
            metadata: frame
                .metadata
                .map(|bytes| ComponentValue::from_schema_bytes(bytes, schema.metadata.clone()))
                .transpose()
                .context("component transition metadata")?,
            identity: Arc::new(()),
        })
    }

    /// Checks persisted named values against the declarations for their original slots.
    fn validate_replay_payloads(&self, schema: &GameSchema) -> Result<()> {
        self.views[0]
            .to_schema_messagepack(&schema.view_a)
            .context("invalid checkpoint role A observation")?;
        self.views[1]
            .to_schema_messagepack(&schema.view_b)
            .context("invalid checkpoint role B observation")?;
        if let Some(completion) = &self.completion {
            completion
                .to_schema_messagepack(&schema.completion)
                .context("invalid checkpoint completion")?;
        }
        if let Some(metadata) = &self.metadata {
            metadata
                .to_schema_messagepack(&schema.metadata)
                .context("invalid checkpoint transition metadata")?;
        }
        Ok(())
    }
}

/// Immutable reconstruction recipe shared by adjacent frames. Successful turns
/// extend the history in constant time; a guest checkpoint starts a new chain.
#[derive(Clone, Serialize, Deserialize)]
struct Recovery {
    origin: Arc<Origin>,
    turns: History,
}

impl Recovery {
    /// Starts a new reconstruction segment from an opaque guest checkpoint.
    fn checkpoint(bytes: Vec<u8>) -> Self {
        Self {
            origin: Arc::new(Origin::Checkpoint(bytes)),
            turns: History::default(),
        }
    }

    /// Appends an accepted action without copying previous turns or the checkpoint.
    fn advance(&self, actor: PlayerRole, action: ComponentValue) -> Self {
        Self {
            origin: self.origin.clone(),
            turns: History(Some(Arc::new(Node {
                turn: Turn { actor, action },
                previous: self.turns.clone(),
            }))),
        }
    }

    /// Rebuilds only on rollback, trap, or historical replay. The original
    /// accepted sequence must still be legal under the pinned component.
    fn restore(&self, guest: &mut Guest, schema: &GameSchema) -> Result<()> {
        let api = guest.game.parlando_game_mechanics();
        guest.metering.begin_call(&mut guest.store)?;
        match self.origin.as_ref() {
            Origin::Seed { config, seed } => {
                let config = config
                    .to_schema_messagepack(&schema.config)
                    .context("encoding component recovery configuration")?;
                api.call_init(&mut guest.store, config.as_ref(), *seed)?
                    .map_err(|e| anyhow!("component recovery initialization: {e}"))?;
            }
            Origin::Checkpoint(bytes) => {
                api.call_restore(&mut guest.store, bytes)?
                    .map_err(|e| anyhow!("component restore: {e}"))?;
            }
        }
        for turn in self.turns.chronological() {
            guest.metering.begin_call(&mut guest.store)?;
            let action = turn
                .action
                .to_schema_messagepack(&schema.action)
                .context("encoding component recovery action")?;
            api.call_step(&mut guest.store, guest_role(turn.actor), action.as_ref())?
                .map_err(|e| anyhow!("component rejected a previously accepted action: {e}"))?;
        }
        Ok(())
    }
}

/// Selects initialization or a guest-defined recovery point.
#[derive(Serialize, Deserialize)]
enum Origin {
    Seed {
        #[serde(with = "recorded_json")]
        config: ComponentValue,
        seed: u64,
    },
    Checkpoint(Vec<u8>),
}

/// Retained action in the compact live reconstruction history.
#[derive(Serialize, Deserialize)]
struct Turn {
    actor: PlayerRole,
    #[serde(with = "recorded_json")]
    action: ComponentValue,
}

/// Keeps recovery configuration and actions in the established JSON-text fields on disk.
mod recorded_json {
    use super::ComponentValue;
    use serde::{Deserialize, Deserializer, Serializer};

    /// Serializes one binary game value as JSON text within the snapshot.
    pub(super) fn serialize<S: Serializer>(
        payload: &ComponentValue,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        // The original recovery format embeds compact JSON text with sorted object keys.
        let recorded = serde_json::to_value(payload).map_err(serde::ser::Error::custom)?;
        serializer.serialize_str(&recorded.to_string())
    }

    /// Reconstitutes a binary game value from its recorded JSON text on load.
    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<ComponentValue, D::Error> {
        let json = String::deserialize(deserializer)?;
        serde_json::from_str(&json).map_err(serde::de::Error::custom)
    }
}

/// Immutable cons list; a serialized frame uses an ordinary chronological array.
#[derive(Clone, Default)]
struct History(Option<Arc<Node>>);

/// One suffix of the action history, shared with the preceding committed frame.
struct Node {
    turn: Turn,
    previous: History,
}

impl History {
    /// Produces storage/replay order only when serializing or rebuilding.
    fn chronological(&self) -> Vec<&Turn> {
        let mut turns = Vec::new();
        let mut node = self.0.as_deref();
        while let Some(current) = node {
            turns.push(&current.turn);
            node = current.previous.0.as_deref();
        }
        turns.reverse();
        turns
    }
}

impl Drop for History {
    /// Drops uniquely owned history iteratively, including after long RL episodes.
    fn drop(&mut self) {
        let mut node = self.0.take();
        while let Some(mut current) = node.and_then(Arc::into_inner) {
            node = current.previous.0.take();
        }
    }
}

impl Serialize for History {
    /// Writes only the retained accepted actions since the last checkpoint.
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        self.chronological().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for History {
    /// Reconstructs shared-list ordering from a stored chronological array.
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        Ok(Vec::<Turn>::deserialize(deserializer)?
            .into_iter()
            .fold(Self::default(), |previous, turn| {
                Self(Some(Arc::new(Node { turn, previous })))
            }))
    }
}

/// Maps Parlando's role enum to the same two seats in the component interface.
fn guest_role(role: PlayerRole) -> Role {
    match role {
        PlayerRole::A => Role::A,
        PlayerRole::B => Role::B,
    }
}

/// A single live or headless session. Only the most recently executed guest is
/// cached. Its state is private until the host installs its returned frame.
pub struct GameSession {
    loaded_game: LoadedGame,
    current: Mutex<Option<(Arc<()>, Guest)>>,
}

/// Store and typed interface for one independently mutable component instance.
struct Guest {
    metering: WasmMetering,
    store: Store<StoreLimits>,
    game: bindings::Game,
}

impl Guest {
    /// Allocates a clean instance, including fresh globals and linear memory.
    fn instantiate(compiled: &InstancePre<StoreLimits>, metering: WasmMetering) -> Result<Self> {
        let limits = StoreLimitsBuilder::new()
            .memory_size(MEMORY_LIMIT)
            .table_elements(65_536)
            .memories(4)
            .tables(8)
            // Language runtimes can bundle several core Wasm modules per session.
            .instances(64)
            .build();
        let mut store = Store::new(compiled.engine(), limits);
        store.limiter(|limits| limits);
        metering.begin_call(&mut store)?;
        let instance = compiled.instantiate(&mut store)?;
        let game = bindings::Game::new(&mut store, &instance)?;
        Ok(Self {
            metering,
            store,
            game,
        })
    }
}

impl GameSession {
    /// Initializes the guest from public configuration and the recorded seed.
    pub fn init(&self, config: &ComponentValue, seed: u64) -> Result<GameFrame> {
        let config_bytes = config
            .to_schema_messagepack(&self.loaded_game.schema().config)
            .context("encoding component configuration")?;
        let mut current = self
            .current
            .lock()
            .map_err(|_| anyhow!("component lock poisoned"))?;
        let mut guest = Guest::instantiate(&self.loaded_game.compiled, self.loaded_game.metering)?;
        guest.metering.begin_call(&mut guest.store)?;
        let frame = guest
            .game
            .parlando_game_mechanics()
            .call_init(&mut guest.store, config_bytes.as_ref(), seed)?
            .map_err(|error| anyhow!("component initialization: {error}"))?;
        let state = GameFrame::from_frame(frame, &self.loaded_game, || Recovery {
            origin: Arc::new(Origin::Seed {
                config: config.clone(),
                seed,
            }),
            turns: History::default(),
        })?;
        *current = Some((state.identity.clone(), guest));
        Ok(state)
    }

    /// Encodes the action for this game's schema before touching the guest.
    /// Reconstructs an uncached committed frame in a fresh instance. A trap or
    /// payload size error drops the potentially mutated instance.
    pub fn step(
        &self,
        state: &GameFrame,
        action: &ComponentValue,
        actor: PlayerRole,
    ) -> Result<std::result::Result<GameFrame, ActionRejection>> {
        ensure!(
            state.component == self.loaded_game.digest,
            "checkpoint belongs to a different game build"
        );
        let schema = self.loaded_game.schema();
        let action_bytes = match action.to_schema_messagepack(&schema.action) {
            Ok(bytes) => bytes,
            Err(_) => return Ok(Err(ActionRejection::new("invalid_component_action"))),
        };
        let mut current = self
            .current
            .lock()
            .map_err(|_| anyhow!("component lock poisoned"))?;
        let mut guest = match current.take() {
            Some((id, guest)) if Arc::ptr_eq(&id, &state.identity) => guest,
            _ => {
                let mut guest =
                    Guest::instantiate(&self.loaded_game.compiled, self.loaded_game.metering)?;
                state.recovery.restore(&mut guest, schema)?;
                guest
            }
        };
        let role = guest_role(actor);
        guest.metering.begin_call(&mut guest.store)?;
        match guest.game.parlando_game_mechanics().call_step(
            &mut guest.store,
            role,
            action_bytes.as_ref(),
        )? {
            Ok(frame) => {
                let next = GameFrame::from_frame(frame, &self.loaded_game, || {
                    state.recovery.advance(actor, action.clone())
                })?;
                *current = Some((next.identity.clone(), guest));
                Ok(Ok(next))
            }
            Err(code) => {
                *current = Some((state.identity.clone(), guest));
                Ok(Err(ActionRejection::new(code)))
            }
        }
    }

    /// Checks the pinned component identity and the persisted named payloads before replay.
    pub fn validate_replay_state(&self, state: &GameFrame) -> Result<()> {
        ensure!(
            state.component == self.loaded_game.digest,
            "checkpoint belongs to a different game build"
        );
        state.validate_replay_payloads(self.loaded_game.schema())
    }
}

impl GameFrame {
    /// Persists only frames whose guest explicitly supplied a complete checkpoint.
    pub fn turn_checkpoint(&self) -> Result<Option<Value>> {
        if self.recovery.turns.0.is_none() && matches!(*self.recovery.origin, Origin::Checkpoint(_))
        {
            Ok(Some(serde_json::to_value(self)?))
        } else {
            Ok(None)
        }
    }

    /// Shares the requested role's cached, JSON-serializable view of the supplied frame.
    pub fn observation(&self, role: PlayerRole) -> ComponentValue {
        self.views[usize::from(role == PlayerRole::B)].clone()
    }

    /// Materializes neutral event metadata when the runtime logs the transition.
    /// Logs invalid output from a nonconforming codec and omits that metadata.
    pub fn transition_metadata(&self) -> Option<Value> {
        let metadata = self.metadata.as_ref()?;
        match serde_json::to_value(metadata) {
            Ok(value) => Some(value),
            Err(error) => {
                tracing::error!(%error, "invalid component transition metadata");
                None
            }
        }
    }

    /// Returns the terminal result from the same step as both role views.
    pub fn completion(&self) -> Option<ComponentValue> {
        self.completion.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parlando_game_sdk::Field;
    use serde_json::json;

    /// Switching the mode changes the engine instrumentation while preserving the package.
    #[test]
    fn wasm_metering_selects_store_instrumentation() {
        let game =
            LoadedGame::from_bytes(include_bytes!("agent_experiment/tests/headless.wasm")).unwrap();
        let digest = game.digest().to_owned();
        let guest = Guest::instantiate(&game.compiled, game.metering).unwrap();
        assert!(guest.store.get_fuel().unwrap() <= CALL_FUEL);
        let schema = game.schema.clone();
        let game = game.with_metering(WasmMetering::None).unwrap();
        assert!(Arc::ptr_eq(&game.schema, &schema));
        assert_eq!(game.digest(), digest);
        let guest = Guest::instantiate(&game.compiled, game.metering).unwrap();
        assert!(guest.store.get_fuel().is_err());
    }

    /// Verifies each named snapshot payload against its own schema before replay.
    #[test]
    fn persisted_frame_payloads_use_their_declared_slots() {
        let view_a = Schema::Struct(vec![Field {
            name: "score".to_owned(),
            schema: Schema::U64,
            omit_none: false,
        }]);
        let schema = GameSchema::new(
            Schema::Null,
            Schema::Null,
            view_a,
            Schema::Bool,
            Schema::String,
            Schema::Null,
        );
        let named = json!({
            "component": "component digest",
            "recovery": {"origin": {"Checkpoint": []}, "turns": []},
            "views": [{"score": 7}, true],
            "completion": "won",
            "metadata": null,
        });
        let state: GameFrame = serde_json::from_value(named.clone()).unwrap();
        state.validate_replay_payloads(&schema).unwrap();
        assert_eq!(serde_json::to_value(state).unwrap(), named);

        let mut invalid = named;
        invalid["views"][1] = json!({"score": 7});
        let state: GameFrame = serde_json::from_value(invalid).unwrap();
        let error = state.validate_replay_payloads(&schema).unwrap_err();
        assert!(error.to_string().contains("checkpoint role B observation"));
    }
}
