#[cfg(not(target_arch = "wasm32"))]
mod agents;
#[cfg(any(not(target_arch = "wasm32"), feature = "agent-component"))]
mod dealer;
mod game;

#[cfg(not(target_arch = "wasm32"))]
pub use agents::CorrectnessReward;
/// Registers the dealer in a native training runner.
#[cfg(not(target_arch = "wasm32"))]
pub type DealerAgentFactory = parlando::agent::native::TypedAgentFactory<dealer::DealerAgent>;
pub use game::{
    Choice, CueChoiceAction, CueChoiceCompletion, CueChoiceConfig, CueChoiceObservation,
    CueChoiceState, Level,
};

#[cfg(feature = "agent-component")]
parlando_game_sdk::export_agent!(dealer::DealerAgent);
