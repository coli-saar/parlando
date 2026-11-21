//! Deterministic rules and component exports for GreatTree.

mod game;

pub use game::adapter::GreatTree;
pub use game::ids::{LimbId, RootId};
pub use game::types::GreatTreeAction as Action;

parlando_game_sdk::export!(GreatTree);

#[cfg(all(feature = "bundled-agents", not(target_arch = "wasm32")))]
#[path = "../../../support/agent_response.rs"]
mod agent_response;
#[cfg(all(feature = "bundled-agents", not(target_arch = "wasm32")))]
mod agents;
#[cfg(all(feature = "bundled-agents", not(target_arch = "wasm32")))]
pub use agents::{IdleAgentFactory, LlmAgentFactory, RootBotAgentFactory};
