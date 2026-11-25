use anyhow::{bail, ensure, Context as _, Result};
use parlando::agent::{grpc::RemoteAgent, Context, Factory};
use parlando::component::{ComponentValue, LoadedGame, WasmMetering};
use parlando::{deployment, GameMetadata, PlayerRole, SessionLimits};
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    future::Future,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex as SyncMutex},
    time::SystemTime,
};
use tokio::sync::Mutex;

mod session;
use session::{Episode, Setup};

/// One immutable compiled game and opponent factory shared by every rollout.
#[pyclass(frozen)]
struct Engine {
    game: LoadedGame,
    opponent: Arc<dyn Factory>,
    settings: Value,
}

const GAME_CACHE_CAPACITY: usize = 16;

/// A compiled package and the filesystem attributes that invalidate its cache entry.
struct CachedGame {
    path: PathBuf,
    revision: PackageRevision,
    metering: WasmMetering,
    game: LoadedGame,
    metadata: GameMetadata,
}

#[derive(Eq, PartialEq)]
struct PackageRevision {
    len: u64,
    modified: SystemTime,
}

impl PackageRevision {
    /// Returns a revision stamp only when the filesystem reports modification time.
    fn read(path: &Path) -> Result<Self> {
        let metadata = std::fs::metadata(path)?;
        Ok(Self {
            len: metadata.len(),
            modified: metadata.modified()?,
        })
    }
}

/// Keeps game compilation shared even when opponent factories have distinct endpoints.
static GAME_CACHE: LazyLock<SyncMutex<VecDeque<CachedGame>>> =
    LazyLock::new(|| SyncMutex::new(VecDeque::new()));

/// Loads and compiles a checked package once per revision and Wasm metering mode.
///
/// Serialization across misses also ensures simultaneous starts of fresh engines do
/// not compile the same package repeatedly. Play does not use the cache lock.
fn load_game(path: &Path, metering: WasmMetering) -> Result<(LoadedGame, GameMetadata)> {
    let path = path.canonicalize()?;
    let Ok(revision) = PackageRevision::read(&path) else {
        return deployment::load_package(&path, metering);
    };
    let mut cache = GAME_CACHE
        .lock()
        .map_err(|_| anyhow::anyhow!("game cache lock poisoned"))?;
    if let Some(index) = cache.iter().position(|entry| {
        entry.path == path && entry.revision == revision && entry.metering == metering
    }) {
        let entry = cache
            .remove(index)
            .expect("cache entry was located under the lock");
        let result = (entry.game.clone(), entry.metadata.clone());
        cache.push_back(entry);
        return Ok(result);
    }
    let (game, metadata) = deployment::load_package(&path, metering)?;
    // If the package changed while it was read, don't associate these bytes
    // with the earlier revision stamp.
    if PackageRevision::read(&path).ok().as_ref() == Some(&revision) {
        if cache.len() == GAME_CACHE_CAPACITY {
            cache.pop_front();
        }
        cache.push_back(CachedGame {
            path,
            revision,
            metering,
            game: game.clone(),
            metadata: metadata.clone(),
        });
    }
    Ok((game, metadata))
}

/// Factory selection and settings using the experiment configuration vocabulary.
#[derive(Deserialize)]
#[serde(tag = "factory", rename_all = "snake_case", deny_unknown_fields)]
enum Opponent {
    Wasm {
        path: String,
        #[serde(default = "empty_object")]
        settings: Value,
    },
    RemoteGrpc {
        endpoint: Option<String>,
        config_yaml: Option<String>,
        auth_token: Option<String>,
        #[serde(default = "empty_object")]
        settings: Value,
    },
}

/// Returns the default JSON mapping for an unconfigured factory.
fn empty_object() -> Value {
    json!({})
}

#[pymethods]
impl Engine {
    /// Compiles an ordinary game ZIP and resolves its chosen opponent.
    #[new]
    #[pyo3(signature = (game_path, opponent_json, wasm_metering=true))]
    fn new(
        py: Python<'_>,
        game_path: String,
        opponent_json: String,
        wasm_metering: bool,
    ) -> PyResult<Self> {
        py.detach(move || {
            let metering = if wasm_metering {
                WasmMetering::Fuel
            } else {
                WasmMetering::None
            };
            let (game, metadata) = load_game(Path::new(&game_path), metering)?;
            let opponent = serde_json::from_str(&opponent_json)?;
            let (opponent, settings): (Arc<dyn Factory>, Value) = match opponent {
                Opponent::Wasm { path, settings } => (
                    Arc::new(deployment::load_agent_package(
                        Path::new(&path),
                        &game,
                        &metadata.id,
                        metering,
                    )?),
                    settings,
                ),
                Opponent::RemoteGrpc {
                    endpoint,
                    config_yaml,
                    settings,
                    auth_token,
                } => {
                    let (factory, settings) =
                        remote_factory(endpoint, config_yaml, settings, auth_token)?;
                    (factory, settings)
                }
            };
            ensure!(settings.is_object(), "opponent settings must be an object");
            let settings = opponent.definition().normalize_settings(&settings)?;
            Ok(Self {
                game,
                opponent,
                settings,
            })
        })
        .map_err(runtime_error)
    }

    /// Configures an independent rollout; reset starts its asynchronous lifecycle.
    #[pyo3(signature = (config_json, seed, learner_role, schedule, limits_json, first_role=None))]
    fn session(
        &self,
        config_json: &str,
        seed: u64,
        learner_role: &str,
        schedule: &str,
        limits_json: &str,
        first_role: Option<&str>,
    ) -> PyResult<Session> {
        let setup = (|| {
            let config: Value =
                serde_json::from_str(config_json).context("invalid game configuration JSON")?;
            let config = ComponentValue::from_typed(&config)?;
            let role = parse_role(learner_role)?;
            let first = first_role
                .map(parse_role)
                .transpose()?
                .unwrap_or(PlayerRole::A);
            let limits: SessionLimits =
                serde_json::from_str(limits_json).context("invalid session limits")?;
            limits.validate()?;
            Setup::new(
                self.game.clone(),
                Arc::clone(&self.opponent),
                Context {
                    role: session::other(role),
                    seed,
                    settings: self.settings.clone(),
                    factory_secrets: Default::default(),
                    agent_instance_secrets: Default::default(),
                    logger: parlando::SessionLogger::headless(),
                },
                config,
                role,
                schedule,
                first,
                limits,
            )
        })()
        .map_err(value_error)?;
        Ok(Session {
            state: Arc::new(Mutex::new(Rollout::Ready(setup))),
        })
    }
}

/// Creates the standard remote factory, separating transport configuration from agent settings.
fn remote_factory(
    endpoint: Option<String>,
    config_yaml: Option<String>,
    settings: Value,
    auth_token: Option<String>,
) -> Result<(Arc<dyn Factory>, Value)> {
    ensure!(settings.is_object(), "opponent settings must be an object");
    let value = if let Some(endpoint) = endpoint {
        ensure!(
            config_yaml.is_none() || settings.as_object().is_some_and(|value| value.is_empty()),
            "provide config_yaml or settings for the remote agent"
        );
        json!({
            "endpoint": endpoint,
            "config_yaml": config_yaml.unwrap_or(serde_json::to_string(&settings)?),
        })
    } else {
        ensure!(config_yaml.is_none(), "config_yaml requires an endpoint");
        ensure!(
            settings.get("endpoint").is_some(),
            "remote opponent requires an endpoint"
        );
        settings
    };
    let factory: Arc<dyn Factory> = match auth_token {
        Some(token) => Arc::new(RemoteAgent::with_auth_token(token)),
        None => Arc::new(RemoteAgent::new()),
    };
    Ok((factory, value))
}

/// Parses the canonical fixed-seat identifiers and their experiment YAML spellings.
fn parse_role(role: &str) -> Result<PlayerRole> {
    match role {
        "A" | "a" | "player_a" => Ok(PlayerRole::A),
        "B" | "b" | "player_b" => Ok(PlayerRole::B),
        _ => bail!("learner and first roles must be A or B"),
    }
}

/// Single-episode lifecycle with a fresh setup for each native session.
enum Rollout {
    Ready(Setup),
    Running(Box<Episode>),
    Closed,
}

/// Shared state for an episode and its serialized operations.
type RolloutHandle = Arc<Mutex<Rollout>>;

/// Python handle for a single episode. Calls run asynchronously on shared Tokio workers.
#[pyclass(frozen)]
struct Session {
    state: RolloutHandle,
}

impl Drop for Session {
    /// Schedules bounded shutdown if Python discards an episode without calling close.
    fn drop(&mut self) {
        spawn_close(Arc::clone(&self.state));
    }
}

#[pymethods]
impl Session {
    /// Starts this episode and returns its first learner observation as JSON.
    fn reset<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let state = Arc::clone(&self.state);
        operation(py, Arc::clone(&state), async move {
            let mut state = state.lock().await;
            if !matches!(*state, Rollout::Ready(_)) {
                return Err(PyRuntimeError::new_err(
                    "this session has already been reset or closed",
                ));
            }
            let Rollout::Ready(setup) = std::mem::replace(&mut *state, Rollout::Closed) else {
                unreachable!()
            };
            let mut episode = Box::new(setup.start().await.map_err(runtime_error)?);
            let observation = episode.advance().await.map_err(runtime_error)?;
            *state = Rollout::Running(episode);
            Ok(observation)
        })
    }

    /// Applies one learner decision and returns the next learner observation as JSON.
    fn step<'py>(&self, py: Python<'py>, action_json: &str) -> PyResult<Bound<'py, PyAny>> {
        let action = serde_json::from_str::<session::Action>(action_json).map_err(value_error)?;
        let decision = action.decision().map_err(value_error)?;
        let state = Arc::clone(&self.state);
        operation(py, Arc::clone(&state), async move {
            let mut state = state.lock().await;
            let Rollout::Running(episode) = &mut *state else {
                return Err(PyRuntimeError::new_err("reset must precede step"));
            };
            episode.step(decision).await.map_err(runtime_error)
        })
    }

    /// Releases an unfinished episode and shuts down its opponent.
    fn close<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let state = Arc::clone(&self.state);
        operation(py, Arc::clone(&state), async move {
            close_rollout(state).await;
            Ok(())
        })
    }
}

/// Finishes an in-flight operation and closes its rollout if its Python waiter goes away.
fn operation<'py, T>(
    py: Python<'py>,
    state: RolloutHandle,
    execute: impl Future<Output = PyResult<T>> + Send + 'static,
) -> PyResult<Bound<'py, PyAny>>
where
    T: for<'a> IntoPyObject<'a> + Send + 'static,
{
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        let mut cleanup = CloseOnCancel(Some(state));
        let result = tokio::spawn(execute)
            .await
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))?;
        cleanup.0.take();
        result
    })
}

/// Schedules cleanup without interrupting a callback or dropping the remote handle.
struct CloseOnCancel(Option<RolloutHandle>);

impl Drop for CloseOnCancel {
    /// Queues cleanup if the operation was interrupted before returning its result.
    fn drop(&mut self) {
        if let Some(state) = self.0.take() {
            spawn_close(state);
        }
    }
}

/// Queues idempotent cleanup behind any in-flight operation on the shared runtime.
fn spawn_close(state: RolloutHandle) {
    pyo3_async_runtimes::tokio::get_runtime().spawn(close_rollout(state));
}

/// Closes an episode and performs its bounded opponent shutdown exactly once.
async fn close_rollout(state: RolloutHandle) {
    let mut state = state.lock().await;
    if let Rollout::Running(mut episode) = std::mem::replace(&mut *state, Rollout::Closed) {
        episode.shutdown().await;
    }
}

/// Reports a native runtime failure with its error chain.
fn runtime_error(error: anyhow::Error) -> PyErr {
    PyRuntimeError::new_err(format!("{error:#}"))
}

/// Reports invalid configuration or a malformed action before entering the runtime.
fn value_error(error: impl std::fmt::Display) -> PyErr {
    PyValueError::new_err(error.to_string())
}

/// Exposes the small binding surface consumed by the Python environment.
#[pymodule]
fn _native(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<Engine>()?;
    module.add_class::<Session>()?;
    Ok(())
}
