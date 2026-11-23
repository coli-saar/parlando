//! Deterministic rules and component exports for GreatTree.

mod game;

pub use game::adapter::GreatTree;
pub use game::ids::{LimbId, RootId};
pub use game::types::GreatTreeAction as Action;

#[cfg(not(any(feature = "root-bot", feature = "idle")))]
parlando_game_sdk::export!(GreatTree);

#[cfg(any(feature = "bundled-agents", feature = "root-bot"))]
#[path = "../../../support/agent_response.rs"]
mod agent_response;
#[cfg(all(feature = "bundled-agents", not(target_arch = "wasm32")))]
mod agents;
#[cfg(any(feature = "bundled-agents", feature = "root-bot", feature = "idle"))]
#[derive(serde::Deserialize)]
/// Both scripted agents accept an empty settings object.
pub struct ScriptedConfig {}

#[cfg(any(feature = "bundled-agents", feature = "idle"))]
mod idle;
#[cfg(any(feature = "bundled-agents", feature = "root-bot"))]
mod root_bot;
#[cfg(all(feature = "bundled-agents", not(target_arch = "wasm32")))]
pub use agents::{LlmAgentFactory, RootBotAgentFactory};
/// Registers the same idle participant exported by the idle component.
#[cfg(all(feature = "bundled-agents", not(target_arch = "wasm32")))]
pub type IdleAgentFactory = parlando::agent::native::TypedAgentFactory<idle::IdleAgent>;

#[cfg(feature = "root-bot")]
parlando_game_sdk::export_agent!(root_bot::RootBotAgent);

#[cfg(feature = "idle")]
parlando_game_sdk::export_agent!(idle::IdleAgent);
