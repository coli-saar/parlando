//! Typed policy decisions are converted to wire values at the host callback boundary.

use parlando::agent;
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
