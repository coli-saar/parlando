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
mod server;
mod storage;
mod transcription;
mod tts;

pub use game::{ActionRejection, Game, GameMetadata, PlayerRole};
pub use server::Server;

/// APIs for implementing optional computer-controlled players.
pub mod agent {
    pub use crate::agents::{
        Agent, AgentContext as Context, AgentFactory as Factory, AgentIdentity as Identity,
        AgentResponse as Response,
    };
    pub use crate::game::{AgentConfigField as ConfigField, AgentDefinition as Definition};

    /// Adapter for agents hosted behind Parlando's versioned gRPC protocol.
    pub mod grpc {
        pub use crate::remote_agent::RemoteGrpcAgentFactory as Factory;
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
    pub use crate::storage::{merge_sqlite_catalogues, CatalogueMergeReport, CatalogueRowCounts};
    pub use crate::transcription::*;
    pub use crate::tts::*;
}
