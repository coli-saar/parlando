//! Deterministic rules and component exports for SpaceGame.

mod game;

pub use game::adapter::SpaceGame;

#[cfg(not(feature = "agent-component"))]
parlando_game_sdk::export!(SpaceGame);

#[cfg(any(feature = "bundled-agents", feature = "agent-component"))]
mod agents;

/// Registers the same Back and forth agent used in the independently built component.
#[cfg(all(feature = "bundled-agents", not(target_arch = "wasm32")))]
pub type BackAndForthAgentFactory =
    parlando::agent::native::TypedAgentFactory<agents::BackAndForthAgent>;

#[cfg(feature = "agent-component")]
parlando_game_sdk::export_agent!(agents::BackAndForthAgent);
