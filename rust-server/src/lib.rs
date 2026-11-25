//! Runs packaged WebAssembly games in live sessions and headless experiments.

mod agent_experiment;
mod agents;
mod app;
mod audio;
mod audio_publisher;
mod auth;
mod canonical_json;
pub mod component;
mod config;
mod consent_templates;
mod imprint_templates;
mod privacy_notice_templates;
mod markdown_templates;
pub mod deployment;
mod game;
mod identity;
mod native_agent;
mod prolific;
mod protocol;
mod readable_id;
mod remote_agent;
mod runtime_limits;
mod server;
mod session_log;
mod storage;
#[cfg(feature = "stress-tui")]
mod stress_dashboard;
mod transcription;
mod tts;
mod wasm_agent;


pub use agent_experiment::{
    AgentSchedule, AlternateAfterAction, AlternateEveryResponse, CheckpointId, ExperimentFile,
    ExperimentRunSummary, ExperimentRunner, RLAgent, RLTrainingContext, ResultAgent,
    RewardFunction, RoleRewards, SessionResult, SessionStatus, TrainingBatch, TrajectoryStep,
};
pub use game::{ActionRejection, GameMetadata, PlayerRole, SecretValues};
pub use runtime_limits::{bundled_runtime_limits, ParticipantCreationLimit, RuntimeLimits};
pub use server::Server;
pub use session_log::{
    SessionLogError, SessionLogger, MAX_SESSION_LOG_BYTES, MAX_SESSION_LOG_ENTRY_BYTES,
    SESSION_LOG_QUEUE_CAPACITY,
};

/// APIs for implementing optional computer-controlled players.
pub mod agent {
    pub use crate::agents::{
        configuration_fingerprint, Agent, AgentContext as Context, AgentFactory as Factory,
        AgentIdentity as Identity, AgentResponse as Response, Decision, Message, Observation,
        Reason, Transition, WakeupRequest,
    };
    pub use crate::game::{
        AgentConfigChoice as Choice, AgentConfigField as ConfigField,
        AgentConfigValue as ConfigValue, AgentDefinition as Definition, SecretPurpose,
        StringFormat,
    };

    /// Adapter that runs a typed Rust policy on the host.
    pub mod native {
        pub use crate::native_agent::TypedAgentFactory;
    }

    /// Factory for locally packaged WASM policies.
    pub mod wasm {
        pub use crate::wasm_agent::{
            WasmAgentCalls, WasmAgentDecisions, WasmAgentFactory, WasmAgentObservations,
            WasmAgentTelemetry, WasmAgentTelemetrySnapshot,
        };
    }

    /// Adapter for agents hosted behind Parlando's versioned gRPC protocol.
    pub mod grpc {
        pub use crate::remote_agent::RemoteAgent;
    }
}

/// Internal hooks compiled only for Parlando's own tools and integration tests.
///
/// This module is absent from normal builds and is not a supported downstream API.
#[cfg(feature = "internal-tools")]
#[doc(hidden)]
pub mod test_support {
    pub use crate::app::{build_game_router, build_router, serve_game, ServeOptions};
    pub use crate::audio::*;
    pub use crate::config::*;
    pub use crate::remote_agent::pb as remote_agent_pb;
    #[cfg(feature = "stress-tui")]
    pub use crate::stress_dashboard::*;
    pub use crate::transcription::*;
    pub use crate::tts::*;
}
