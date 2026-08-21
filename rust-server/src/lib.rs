mod agent_experiment;
mod agents;
mod app;
mod audio;
mod audio_publisher;
mod auth;
mod config;
mod game;
mod identity;
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

pub use agent_experiment::{
    AgentSchedule, AlternateAfterAction, AlternateEveryResponse, CheckpointId,
    ExperimentRunSummary, ExperimentRunner, RLAgent, RLTrainingContext, ResultAgent,
    RewardFunction, RoleRewards, SessionResult, SessionStatus, TrainingBatch, TrajectoryStep,
};
pub use game::{
    ActionRejection, Game, GameFactory, GameInitializationContext, GameMetadata,
    GameSessionContext, PlayerRole, SecretValues,
};
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
        AgentIdentity as Identity, AgentResponse as Response,
    };
    pub use crate::game::{
        AgentConfigChoice as Choice, AgentConfigField as ConfigField,
        AgentConfigValue as ConfigValue, AgentDefinition as Definition, SecretPurpose,
        StringFormat,
    };

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
