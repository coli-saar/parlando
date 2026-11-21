//! Deterministic rules and component exports for SpaceGame.

mod game;

pub use game::adapter::SpaceGame;

parlando_game_sdk::export!(SpaceGame);

#[cfg(all(feature = "bundled-agents", not(target_arch = "wasm32")))]
#[path = "../../../games/support/agent_response.rs"]
mod agent_response;
#[cfg(all(feature = "bundled-agents", not(target_arch = "wasm32")))]
mod agents;
#[cfg(all(feature = "bundled-agents", not(target_arch = "wasm32")))]
pub use agents::BackAndForthAgentFactory;
