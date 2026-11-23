//! Hosts import-free agent components with the same compact payloads as the game.

use std::{
    path::Path,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use anyhow::{anyhow, ensure, Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use wasmtime::{
    component::{Component, Linker},
    Config, Engine, Store, StoreLimits, StoreLimitsBuilder,
};

use crate::{
    agents::{
        Agent, AgentContext, AgentFactory, AgentIdentity, Decision, Observation, Reason,
        WakeupRequest,
    },
    component::{ComponentValue, LoadedGame, Schema, WasmMetering},
    game::AgentDefinition,
    native_agent::PolicyLayouts,
    PlayerRole,
};

mod bindings {
    wasmtime::component::bindgen!({
        path: "wit/agent",
        world: "agent",
        ownership: Borrowing { duplicate_if_necessary: true },
    });
}

use bindings::exports::parlando::agent::policy as wire;

#[derive(Deserialize)]
struct Description {
    definition: AgentDefinition,
    version: String,
    action: Schema,
    view: Schema,
    completion: Schema,
}

/// Validated component metadata used to choose a compatible game without recompiling.
pub(crate) struct InspectedAgent {
    description: Description,
    pub(crate) identity: AgentIdentity,
}

impl InspectedAgent {
    /// Exposes the definition declared by the component's discovery export.
    pub(crate) fn definition(&self) -> &AgentDefinition {
        &self.description.definition
    }
}

/// Maps the policy declarations to the game's canonical compact layout handles.
fn layouts(description: &Description, game: &LoadedGame) -> Result<PolicyLayouts> {
    PolicyLayouts::for_game(
        game,
        &description.action,
        &description.view,
        &description.completion,
    )
}

/// Optional byte and callback counters shared by the component factories of a run.
#[derive(Default)]
pub struct WasmAgentTelemetry {
    starts: Calls,
    observations: [Calls; 4],
    decisions: [Calls; 4],
    finishes: Calls,
    #[cfg(test)]
    store_fuel_modes: std::sync::Mutex<Vec<bool>>,
}

impl WasmAgentTelemetry {
    /// Reports whether fuel accounting is available on each created session store.
    #[cfg(test)]
    pub(crate) fn store_fuel_modes(&self) -> Vec<bool> {
        self.store_fuel_modes.lock().unwrap().clone()
    }

    /// Takes a snapshot, typically after all sessions have completed.
    pub fn snapshot(&self) -> WasmAgentTelemetrySnapshot {
        let [empty, transition, message, combined] =
            self.observations.each_ref().map(Calls::snapshot);
        let observations = WasmAgentObservations {
            transition,
            message,
            combined,
            empty,
        };
        let [empty, action, message, combined] = self.decisions.each_ref().map(Calls::snapshot);
        let decisions = WasmAgentDecisions {
            action,
            message,
            combined,
            empty,
        };
        WasmAgentTelemetrySnapshot {
            start: self.starts.snapshot(),
            observations,
            decisions,
            finish: self.finishes.snapshot(),
        }
    }
}

/// Payload totals from the component boundary, excluding ABI framing and allocation.
#[derive(Debug, Serialize)]
pub struct WasmAgentTelemetrySnapshot {
    /// Initial view deliveries.
    pub start: WasmAgentCalls,
    /// Observation deliveries by the combination of fields present.
    pub observations: WasmAgentObservations,
    /// Returned decisions by the combination of action and message present.
    pub decisions: WasmAgentDecisions,
    /// Completion deliveries.
    pub finish: WasmAgentCalls,
}

/// Observation totals grouped by their optional transition and message fields.
#[derive(Debug, Serialize)]
pub struct WasmAgentObservations {
    /// Accepted action and view deliveries.
    pub transition: WasmAgentCalls,
    /// Player messages.
    pub message: WasmAgentCalls,
    /// Accepted transitions and player messages delivered together.
    pub combined: WasmAgentCalls,
    /// Observations with neither field present.
    pub empty: WasmAgentCalls,
}

/// Decision totals grouped by their optional action and message fields.
#[derive(Debug, Serialize)]
pub struct WasmAgentDecisions {
    /// Action-only decisions.
    pub action: WasmAgentCalls,
    /// Message-only decisions.
    pub message: WasmAgentCalls,
    /// Decisions with both an action and a message.
    pub combined: WasmAgentCalls,
    /// Decisions with neither an action nor a message.
    pub empty: WasmAgentCalls,
}

/// Number and payload size of callbacks in one category.
#[derive(Debug, Serialize)]
pub struct WasmAgentCalls {
    /// Number of completed calls.
    pub calls: u64,
    /// Sum of the lengths of the compact payload and UTF-8 text fields.
    pub bytes: u64,
    /// Total elapsed nanoseconds in these adapter callbacks.
    pub elapsed_ns: u64,
    /// Elapsed nanoseconds spent inside Wasmtime, including ABI payload transfer.
    pub guest_elapsed_ns: u64,
}

#[derive(Default)]
struct Calls {
    calls: AtomicU64,
    bytes: AtomicU64,
    elapsed_ns: AtomicU64,
    guest_elapsed_ns: AtomicU64,
}

impl Calls {
    /// Counts one completed callback and its active payload bytes.
    fn record(&self, bytes: usize, elapsed: Duration, guest_elapsed: Duration) {
        self.bytes.fetch_add(bytes as u64, Ordering::Relaxed);
        self.elapsed_ns
            .fetch_add(elapsed.as_nanos() as u64, Ordering::Relaxed);
        self.guest_elapsed_ns
            .fetch_add(guest_elapsed.as_nanos() as u64, Ordering::Relaxed);
        self.calls.fetch_add(1, Ordering::Relaxed);
    }

    /// Records host and guest durations with the same completion timestamp.
    fn record_timed(&self, bytes: usize, started: Instant, guest_started: Instant) {
        let finished = Instant::now();
        self.record(
            bytes,
            finished.duration_since(started),
            finished.duration_since(guest_started),
        );
    }

    /// Reads the callback counters without synchronizing live session workers.
    fn snapshot(&self) -> WasmAgentCalls {
        WasmAgentCalls {
            calls: self.calls.load(Ordering::Relaxed),
            bytes: self.bytes.load(Ordering::Relaxed),
            elapsed_ns: self.elapsed_ns.load(Ordering::Relaxed),
            guest_elapsed_ns: self.guest_elapsed_ns.load(Ordering::Relaxed),
        }
    }
}

/// Selects the two-bit category for the first and second optional fields.
fn category(first: bool, second: bool) -> usize {
    usize::from(first) + 2 * usize::from(second)
}

/// A precompiled policy component and its checked contract with one game.
///
/// Each agent created by this factory owns a fresh Wasmtime store. The factory
/// shares compiled code, the discovered agent definition, and the game's wire
/// declarations across sessions.
pub struct WasmAgentFactory {
    compiled: bindings::AgentPre<StoreLimits>,
    bytes: Arc<[u8]>,
    metering: WasmMetering,
    diagnostic_metering: bool,
    definition: AgentDefinition,
    identity: AgentIdentity,
    digest: String,
    layouts: PolicyLayouts,
    telemetry: Option<Arc<WasmAgentTelemetry>>,
}

impl WasmAgentFactory {
    /// Loads a component under the same instrumentation as the specified game.
    pub fn load(path: &Path, game: &LoadedGame) -> Result<Self> {
        Self::load_with_metering(path, game, game.metering())
    }

    /// Compiles and describes the policy once, verifying that it accepts the
    /// game's action, completion, and at least one seat's observation layout.
    ///
    /// Choose fuel metering when running untrusted policy code. Without it,
    /// a guest that loops indefinitely can occupy the caller's worker.
    pub fn load_with_metering(
        path: &Path,
        game: &LoadedGame,
        metering: WasmMetering,
    ) -> Result<Self> {
        (|| {
            let bytes = std::fs::read(path)?;
            Self::from_bytes_with_metering(&bytes, game, metering)
        })()
        .with_context(|| format!("loading agent from {}", path.display()))
    }

    /// Keeps this factory's current instrumentation for diagnostic comparisons.
    #[doc(hidden)]
    pub fn with_diagnostic_metering(mut self) -> Self {
        self.diagnostic_metering = true;
        self
    }

    /// Enables optional callback and payload counts for instances from this factory.
    pub fn with_telemetry(mut self, telemetry: Arc<WasmAgentTelemetry>) -> Self {
        self.telemetry = Some(telemetry);
        self
    }

    /// Returns the SHA-256 digest of the executable bytes loaded by this factory.
    pub fn digest(&self) -> &str {
        &self.digest
    }

    /// Compiles the pinned policy bytes with the selected store instrumentation.
    fn compile_component(
        bytes: &[u8],
        metering: WasmMetering,
    ) -> Result<bindings::AgentPre<StoreLimits>> {
        let mut config = Config::new();
        config.consume_fuel(metering.is_fuel());
        let engine = Engine::new(&config)?;
        let component = Component::from_binary(&engine, bytes)?;
        Ok(bindings::AgentPre::new(
            Linker::new(&engine).instantiate_pre(&component)?,
        )?)
    }

    /// Reads the definition and identity of a policy before choosing its game.
    pub(crate) fn inspect_bytes_with_metering(
        bytes: &[u8],
        metering: WasmMetering,
    ) -> Result<InspectedAgent> {
        let compiled = Self::compile_component(bytes, metering)?;
        let (description, identity) = Self::describe(&compiled, metering)?;
        description.action.validate()?;
        description.view.validate()?;
        description.completion.validate()?;
        Ok(InspectedAgent {
            description,
            identity,
        })
    }

    /// Reads description metadata through a bounded, isolated discovery instance.
    fn describe(
        compiled: &bindings::AgentPre<StoreLimits>,
        metering: WasmMetering,
    ) -> Result<(Description, AgentIdentity)> {
        let (mut store, instance) = instantiate(compiled, metering)?;
        metering.begin_call(&mut store)?;
        let description =
            guest_result(instance.parlando_agent_policy().call_describe(&mut store)?)?;
        let description: Description =
            serde_json::from_str(&description).context("decoding the agent description")?;
        description.definition.validate()?;
        let identity = AgentIdentity {
            name: description.definition.name.clone(),
            version: description.version.clone(),
        };
        identity.validate()?;
        Ok((description, identity))
    }

    /// Loads a policy from validated package bytes and verifies its types against the game.
    pub(crate) fn from_bytes_with_metering(
        bytes: &[u8],
        game: &LoadedGame,
        metering: WasmMetering,
    ) -> Result<Self> {
        let compiled = Self::compile_component(bytes, metering)?;
        let (description, identity) = Self::describe(&compiled, metering)?;
        let layouts = layouts(&description, game)?;

        Ok(Self {
            compiled,
            bytes: bytes.into(),
            metering,
            diagnostic_metering: false,
            definition: description.definition,
            identity,
            digest: format!("{:x}", Sha256::digest(bytes)),
            layouts,
            telemetry: None,
        })
    }
}

#[async_trait]
impl AgentFactory for WasmAgentFactory {
    /// Returns the dashboard definition provided by the component.
    fn definition(&self) -> AgentDefinition {
        self.definition.clone()
    }

    /// Recompiles pinned bytes once when the run selects a different instrumentation mode.
    fn with_host_wasm_metering(
        &self,
        metering: WasmMetering,
    ) -> Result<Option<Arc<dyn AgentFactory>>> {
        if self.metering == metering || self.diagnostic_metering {
            return Ok(None);
        }
        let compiled = Self::compile_component(&self.bytes, metering).with_context(|| {
            format!("configuring WASM metering for agent {}", self.definition.id)
        })?;
        Ok(Some(Arc::new(Self {
            compiled,
            bytes: self.bytes.clone(),
            metering,
            diagnostic_metering: false,
            definition: self.definition.clone(),
            identity: self.identity.clone(),
            digest: self.digest.clone(),
            layouts: self.layouts.clone(),
            telemetry: self.telemetry.clone(),
        })))
    }

    /// Creates an isolated guest and delivers the assigned role, settings, and seed.
    async fn create(&self, context: AgentContext) -> Result<Box<dyn Agent + Send>> {
        ensure!(
            context.factory_secrets.is_empty() && context.agent_instance_secrets.is_empty(),
            "WASM agents do not receive secrets"
        );
        let view = self.layouts.view(context.role)?;
        let (mut store, instance) = instantiate(&self.compiled, self.metering)?;
        #[cfg(test)]
        if let Some(telemetry) = &self.telemetry {
            telemetry
                .store_fuel_modes
                .lock()
                .unwrap()
                .push(store.get_fuel().is_ok());
        }
        let config = ComponentValue::from_typed(&context.settings)?;
        self.metering.begin_call(&mut store)?;
        guest_result(instance.parlando_agent_policy().call_init(
            &mut store,
            config.as_messagepack(),
            lower_role(context.role),
            context.seed,
        )?)
        .context("initializing WASM agent")?;
        Ok(Box::new(WasmAgent {
            store,
            instance,
            metering: self.metering,
            action: Arc::clone(&self.layouts.action),
            view: Arc::clone(view),
            completion: Arc::clone(&self.layouts.completion),
            telemetry: self.telemetry.clone(),
        }))
    }

    /// Pins this policy in experiment provenance independently of its semantic version.
    fn wasm_sha256(&self) -> Option<&str> {
        Some(self.digest())
    }

    /// Returns the release identity declared in the compiled component.
    fn identity(&self, _settings: &Value) -> Result<AgentIdentity> {
        Ok(self.identity.clone())
    }
}

/// Allocates a new isolated guest with no host imports and bounded linear memory.
fn instantiate(
    compiled: &bindings::AgentPre<StoreLimits>,
    metering: WasmMetering,
) -> Result<(Store<StoreLimits>, bindings::Agent)> {
    let limits = StoreLimitsBuilder::new()
        .memory_size(64 * 1024 * 1024)
        .table_elements(65_536)
        .memories(4)
        .tables(8)
        .instances(64)
        .build();
    let mut store = Store::new(compiled.engine(), limits);
    store.limiter(|limits| limits);
    metering.begin_call(&mut store)?;
    let instance = compiled.instantiate(&mut store)?;
    Ok((store, instance))
}

/// Owns the callbacks, private store, and checked layouts for one policy instance.
struct WasmAgent {
    store: Store<StoreLimits>,
    instance: bindings::Agent,
    metering: WasmMetering,
    action: Arc<Schema>,
    view: Arc<Schema>,
    completion: Arc<Schema>,
    telemetry: Option<Arc<WasmAgentTelemetry>>,
}

#[async_trait]
impl Agent for WasmAgent {
    /// Borrows the initial game observation in the seat's compact layout.
    async fn start(&mut self, view: ComponentValue) -> Result<()> {
        let started = self.telemetry.as_ref().map(|_| Instant::now());
        let view = view.to_schema_messagepack(&self.view)?;
        self.metering.begin_call(&mut self.store)?;
        let guest_started = started.map(|_| Instant::now());
        guest_result(
            self.instance
                .parlando_agent_policy()
                .call_start(&mut self.store, &view)?,
        )?;
        if let (Some(telemetry), Some(started), Some(guest_started)) =
            (&self.telemetry, started, guest_started)
        {
            telemetry
                .starts
                .record_timed(view.len(), started, guest_started);
        }
        Ok(())
    }

    /// Sends accepted game and messaging events in one boundary call.
    async fn observe(&mut self, observation: Observation) -> Result<()> {
        let started = self.telemetry.as_ref().map(|_| Instant::now());
        let compact = observation
            .transition
            .as_ref()
            .map(|transition| {
                Ok::<_, anyhow::Error>((
                    transition.action.to_schema_messagepack(&self.action)?,
                    transition.view.to_schema_messagepack(&self.view)?,
                ))
            })
            .transpose()?;
        let transition = observation.transition.as_ref().zip(compact.as_ref()).map(
            |(transition, (action, view))| wire::Transition {
                actor: lower_role(transition.actor),
                action,
                view,
            },
        );
        let message = observation.message.as_ref().map(|message| wire::Message {
            sender: lower_role(message.sender),
            text: &message.text,
        });
        let observation = wire::Observation {
            transition,
            message,
        };

        let traffic = self.telemetry.as_ref().map(|telemetry| {
            let index = category(
                observation.transition.is_some(),
                observation.message.is_some(),
            );
            let bytes = observation.transition.as_ref().map_or(0, |transition| {
                transition.action.len() + transition.view.len()
            }) + observation
                .message
                .as_ref()
                .map_or(0, |message| message.text.len());
            (&telemetry.observations[index], bytes)
        });
        self.metering.begin_call(&mut self.store)?;
        let guest_started = started.map(|_| Instant::now());
        guest_result(
            self.instance
                .parlando_agent_policy()
                .call_observe(&mut self.store, observation)?,
        )?;
        if let (Some((totals, bytes)), Some(started), Some(guest_started)) =
            (traffic, started, guest_started)
        {
            totals.record_timed(bytes, started, guest_started);
        }
        Ok(())
    }

    /// Requests one decision and retains compact action bytes for the game.
    async fn decide(&mut self, reason: Reason) -> Result<Decision> {
        let started = self.telemetry.as_ref().map(|_| Instant::now());
        let reason = match &reason {
            Reason::SessionStarted => wire::RespondReason::SessionStarted,
            Reason::AfterObservation => wire::RespondReason::AfterObservation,
            Reason::Scheduled => wire::RespondReason::Scheduled,
            Reason::WakeupDue => wire::RespondReason::WakeupDue,
            Reason::ActionRejected(code) => wire::RespondReason::ActionRejected(code),
        };
        self.metering.begin_call(&mut self.store)?;
        let guest_started = started.map(|_| Instant::now());
        let outcome = guest_result(
            self.instance
                .parlando_agent_policy()
                .call_respond(&mut self.store, reason)?,
        )?;
        let traffic =
            self.telemetry
                .as_ref()
                .zip(guest_started)
                .map(|(telemetry, guest_started)| {
                    let guest_elapsed = guest_started.elapsed();
                    let index = category(outcome.action.is_some(), outcome.message.is_some());
                    let bytes = outcome.action.as_ref().map_or(0, Vec::len)
                        + outcome.message.as_ref().map_or(0, String::len);
                    (&telemetry.decisions[index], bytes, guest_elapsed)
                });
        let action = outcome
            .action
            .map(|bytes| ComponentValue::from_schema_bytes(bytes, Arc::clone(&self.action)))
            .transpose()?;
        let wakeup = outcome
            .wakeup
            .map(|request| match request {
                wire::WakeupRequest::After(duration) => {
                    ensure!(
                        duration.nanoseconds < 1_000_000_000,
                        "agent wakeup nanoseconds out of range"
                    );
                    Ok(WakeupRequest::After(Duration::new(
                        duration.seconds,
                        duration.nanoseconds,
                    )))
                }
                wire::WakeupRequest::Cancel => Ok(WakeupRequest::Cancel),
            })
            .transpose()?;
        if let (Some((totals, bytes, guest_elapsed)), Some(started)) = (traffic, started) {
            totals.record(bytes, started.elapsed(), guest_elapsed);
        }
        Ok(Decision {
            action,
            message: outcome.message,
            wakeup,
        })
    }

    /// Borrows the game's compact terminal payload.
    async fn finish(&mut self, completion: ComponentValue) -> Result<()> {
        let started = self.telemetry.as_ref().map(|_| Instant::now());
        let completion = completion.to_schema_messagepack(&self.completion)?;
        self.metering.begin_call(&mut self.store)?;
        let guest_started = started.map(|_| Instant::now());
        guest_result(
            self.instance
                .parlando_agent_policy()
                .call_finish(&mut self.store, &completion)?,
        )?;
        if let (Some(telemetry), Some(started), Some(guest_started)) =
            (&self.telemetry, started, guest_started)
        {
            telemetry
                .finishes
                .record_timed(completion.len(), started, guest_started);
        }
        Ok(())
    }

    /// Runs guest cleanup before dropping the Wasmtime store.
    async fn shutdown(&mut self) -> Result<()> {
        self.metering.begin_call(&mut self.store)?;
        guest_result(
            self.instance
                .parlando_agent_policy()
                .call_shutdown(&mut self.store)?,
        )
    }
}

/// Translates the runtime's two fixed seats to the policy ABI.
fn lower_role(role: PlayerRole) -> wire::Role {
    match role {
        PlayerRole::A => wire::Role::A,
        PlayerRole::B => wire::Role::B,
    }
}

/// Surfaces a recoverable guest error through the same callback contract as native agents.
fn guest_result<T>(result: std::result::Result<T, String>) -> Result<T> {
    result.map_err(|message| anyhow!(message))
}
