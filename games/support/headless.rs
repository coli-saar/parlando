//! Runs the production experiment runner with native and WASM policy backends.

use anyhow::Result;
use clap::{Parser, ValueEnum};
use parlando::{
    agent::{
        wasm::{WasmAgentFactory, WasmAgentTelemetry},
        Agent, Context, Definition, Factory, Identity,
    },
    component::{LoadedGame, WasmMetering},
    ExperimentFile, ExperimentRunner, GameMetadata,
};
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc, time::Instant};

/// Selects whether the WASM runtime counts instructions.
#[derive(Clone, Copy, ValueEnum)]
enum Metering {
    Fuel,
    None,
}

impl From<Metering> for WasmMetering {
    /// Maps the launcher flag to the host's execution mode.
    fn from(mode: Metering) -> Self {
        match mode {
            Metering::Fuel => Self::Fuel,
            Metering::None => Self::None,
        }
    }
}

/// Selects the game artifact, experiment batch and optional WASM policy backend.
#[derive(Parser)]
struct Launch {
    /// Packaged game component.
    #[arg(long)]
    game: PathBuf,
    /// Experiment YAML specification.
    #[arg(long)]
    experiment: PathBuf,
    /// Agent component. Omitting it selects the corresponding native policy.
    #[arg(long)]
    agent: Option<PathBuf>,
    /// Metering used by the WASM agent, independent of the game's setting.
    #[arg(long, value_enum, requires = "agent")]
    agent_metering: Option<Metering>,
    /// Records data exchanged at the WASM policy boundary.
    #[arg(long, requires = "agent")]
    diagnostics: bool,
    /// Makes the native factory available under the policy ID with a `.native` suffix.
    #[arg(long, requires = "agent")]
    mixed: bool,
}

/// Registers a native policy alongside the component under a distinct ID.
struct NativeAlias<F>(F);

#[parlando_game_sdk::agent::async_trait]
impl<F: Factory> Factory for NativeAlias<F> {
    /// Adds the native suffix while preserving the policy's settings schema.
    fn definition(&self) -> Definition {
        let mut definition = self.0.definition();
        definition.id.push_str(".native");
        definition
    }

    /// Gives each native seat a new instance with its assigned context.
    async fn create(&self, context: Context) -> Result<Box<dyn Agent + Send>> {
        self.0.create(context).await
    }

    /// Preserves the policy's implementation name and version.
    fn identity(&self, settings: &Value) -> Result<Identity> {
        self.0.identity(settings)
    }
}

/// Registers the selected policy backends and prints startup and batch timing as JSON.
pub(super) async fn run<F: Factory>(
    id: &str,
    name: &str,
    version: &str,
    native_factory: impl FnOnce(&LoadedGame) -> Result<F>,
) -> Result<()> {
    let args = Launch::parse();
    let spec = ExperimentFile::read(&args.experiment).await?;
    let start = Instant::now();
    let game = LoadedGame::load_with_metering(&args.game, spec.wasm_metering())?;
    let metadata = GameMetadata {
        id: id.into(),
        name: name.into(),
        version: version.parse()?,
        build_manifest: Value::Null,
    };
    let agent_factory = args
        .agent
        .as_ref()
        .map(|path| match args.agent_metering {
            Some(mode) => Ok(
                WasmAgentFactory::load_with_metering(path, &game, mode.into())?
                    .with_diagnostic_metering(),
            ),
            None => WasmAgentFactory::load(path, &game),
        })
        .transpose()?;
    let telemetry = args
        .diagnostics
        .then(|| Arc::new(WasmAgentTelemetry::default()));
    let runner = match agent_factory {
        Some(wasm) => {
            let factory = match &telemetry {
                Some(telemetry) => wasm.with_telemetry(Arc::clone(telemetry)),
                None => wasm,
            };
            let native_alias = args
                .mixed
                .then(|| native_factory(&game).map(NativeAlias))
                .transpose()?;
            let runner = ExperimentRunner::new(metadata, game)?.agent(factory)?;
            match native_alias {
                Some(native) => runner.agent(native)?,
                None => runner,
            }
        }
        None => {
            let factory = native_factory(&game)?;
            ExperimentRunner::new(metadata, game)?.agent(factory)?
        }
    };
    let load_ms = start.elapsed().as_secs_f64() * 1000.0;
    let start = Instant::now();
    let summary = runner.run_file(spec).await?;
    let mut report = json!({
        "load_ms": load_ms,
        "seconds": start.elapsed().as_secs_f64(),
        "summary": summary,
    });
    if let Some(telemetry) = telemetry {
        report["diagnostics"] = serde_json::to_value(telemetry.snapshot())?;
    }
    println!("{report}");
    Ok(())
}
