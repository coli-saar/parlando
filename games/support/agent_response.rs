//! Typed policy decisions are converted to wire values at the host callback boundary.

#[cfg(all(feature = "bundled-agents", not(target_arch = "wasm32")))]
use parlando::agent;
#[cfg(all(feature = "bundled-agents", not(target_arch = "wasm32")))]
use parlando::component::ComponentValue;
use serde::Serialize;

#[derive(Debug, PartialEq)]
pub(crate) enum Response<A> {
    Action(A),
    Message(String),
    ActionAndMessage { action: A, message: String },
}

impl<A: Serialize> Response<A> {
    /// Queues a game operation.
    pub(crate) fn action(action: A) -> Self {
        Self::Action(action)
    }

    /// Queues participant chat.
    pub(crate) fn message(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }

    /// Queues a game operation and a message delivered only on acceptance.
    pub(crate) fn action_and_message(action: A, message: impl Into<String>) -> Self {
        Self::ActionAndMessage {
            action,
            message: message.into(),
        }
    }

    /// Serializes the typed operation using its public wire representation.
    #[cfg(all(feature = "bundled-agents", not(target_arch = "wasm32")))]
    pub(crate) fn into_wire(self) -> anyhow::Result<agent::Response> {
        Ok(match self {
            Self::Action(action) => agent::Response::action(ComponentValue::from_typed(&action)?),
            Self::Message(message) => agent::Response::message(message),
            Self::ActionAndMessage { action, message } => {
                agent::Response::action_and_message(ComponentValue::from_typed(&action)?, message)
            }
        })
    }
}

impl<A> From<Response<A>> for parlando_game_sdk::agent::Response<A> {
    /// Carries a decision from the strategy to the component callback.
    fn from(value: Response<A>) -> Self {
        match value {
            Response::Action(action) => Self::action(action),
            Response::Message(text) => Self::message(text),
            Response::ActionAndMessage { action, message } => {
                Self::action_and_message(action, message)
            }
        }
    }
}
