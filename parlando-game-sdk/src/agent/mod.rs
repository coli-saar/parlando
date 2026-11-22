//! Write a typed policy once, call it directly from Rust tests, and export it as
//! an import-free Parlando agent component for live and headless experiments.

pub use crate::export_agent;
use crate::DecodeGameValue;
pub use crate::Role;
pub use anyhow::Result;
pub use async_trait::async_trait;
use serde::de::DeserializeOwned;
use std::time::Duration;

mod view;
#[doc(hidden)]
pub use view::ViewSource;
pub use view::{AgentView, LazyView};

#[cfg(target_arch = "wasm32")]
#[doc(hidden)]
pub mod component;

#[cfg(target_arch = "wasm32")]
#[doc(hidden)]
pub mod __bindings {
    wit_bindgen::generate!({
        path: "wit/agent",
        world: "agent",
        pub_export_macro: true,
    });
}

/// Describes the policy shown in Parlando and identifies its implementation release.
#[derive(Clone, Debug)]
pub struct Definition {
    /// Stable selector used in experiment configuration.
    pub id: &'static str,
    /// Display name used in the administration dashboard and participant records.
    pub name: &'static str,
    /// Stable semantic release identifier of this implementation.
    pub version: &'static str,
    /// User-facing summary of the policy.
    pub description: &'static str,
    /// JSON array of Parlando agent configuration fields. Use `"[]"` for no fields.
    pub config_fields: &'static str,
}

/// An accepted action and the resulting complete role-private view.
#[derive(Clone, Debug)]
pub struct Transition<A, V> {
    /// Seat that submitted the action.
    pub actor: Role,
    /// Action accepted by the game.
    pub action: A,
    /// Complete view available to the agent after the action.
    pub view: V,
}

/// A player message delivered to the policy.
#[derive(Clone, Debug)]
pub struct Message {
    /// Seat that sent the message.
    pub sender: Role,
    /// Player-visible text.
    pub text: String,
}

/// Game and player events delivered together before a decision.
///
/// If both fields are present, the game transition precedes the message.
#[derive(Clone, Debug)]
pub struct Observation<A, V> {
    /// Accepted game transition, if any.
    pub transition: Option<Transition<A, V>>,
    /// Player message, if any.
    pub message: Option<Message>,
}

impl<A, V> Default for Observation<A, V> {
    /// Creates an observation with no game transition or player message.
    fn default() -> Self {
        Self {
            transition: None,
            message: None,
        }
    }
}

/// Why Parlando made this policy eligible to respond.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RespondReason {
    /// The initial view has been delivered.
    SessionStarted,
    /// An updated view or player message has been delivered.
    AfterObservation,
    /// The headless schedule selected this role.
    Scheduled,
    /// A previously requested wakeup is due.
    WakeupDue,
    /// The previous action was rejected with this game-defined code.
    ActionRejected(String),
}

/// Updates the one pending wakeup for this policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WakeupRequest {
    /// Replaces the pending wakeup with a request after this delay.
    After(Duration),
    /// Clears the pending wakeup.
    Cancel,
}

/// One decision's independent outputs. The runtime applies actions under the game's rules.
#[derive(Clone, Debug)]
pub struct Response<A> {
    /// Action proposed to the game.
    pub action: Option<A>,
    /// Message sent to the other player.
    pub message: Option<String>,
    /// Requested change to the agent's wakeup.
    pub wakeup: Option<WakeupRequest>,
}

impl<A> Default for Response<A> {
    /// Yields without proposing an action, sending a message, or replacing a wakeup.
    fn default() -> Self {
        Self {
            action: None,
            message: None,
            wakeup: None,
        }
    }
}

impl<A> Response<A> {
    /// Proposes one game action.
    pub fn action(action: A) -> Self {
        Self {
            action: Some(action),
            ..Self::default()
        }
    }

    /// Sends a message to the other player.
    pub fn message(message: impl Into<String>) -> Self {
        Self {
            message: Some(message.into()),
            ..Self::default()
        }
    }

    /// Proposes an action and sends a message if the host accepts the action.
    pub fn action_and_message(action: A, message: impl Into<String>) -> Self {
        Self {
            action: Some(action),
            message: Some(message.into()),
            wakeup: None,
        }
    }

    /// Replaces the pending wakeup with a request after the specified delay.
    pub fn wake_after(mut self, delay: Duration) -> Self {
        self.wakeup = Some(WakeupRequest::After(delay));
        self
    }
}

/// Configuration, role, and recorded randomness supplied to one agent.
#[derive(Clone, Debug)]
pub struct InitContext<C> {
    /// Settings decoded from the dashboard configuration object.
    pub config: C,
    /// Seat controlled by this policy.
    pub role: Role,
    /// Recorded seed for deterministic policy randomness.
    pub seed: u64,
}

/// State and callbacks of a policy for one player in one session.
///
/// Match the action, view, and completion types to the game's declared
/// compact layouts. Use the game SDK's built-in types or types implementing
/// its `GameValue` derive. Settings use their normal Serde representation.
/// Pass the implementation to [`export_agent!`] at the crate root.
///
/// Annotate the implementation with [`async_trait`]. Native policies may
/// await asynchronous work. Component callbacks must be ready within one
/// poll because the guest has no host imports. Request a wakeup to defer a
/// later decision or use the gRPC host adapter for external I/O.
#[async_trait]
pub trait Agent: Send + Sized + 'static {
    /// Dashboard settings decoded once before the first view.
    ///
    /// Configurations with no fields can use an empty braced struct.
    type Config: DeserializeOwned + Send;
    /// Game action exchanged in the game's compact representation.
    type Action: for<'de> DecodeGameValue<'de> + Send;
    /// Complete role-private observation from the game, optionally decoded on demand.
    type View: AgentView;
    /// Terminal result shared by both players.
    type Completion: for<'de> DecodeGameValue<'de> + Send;

    /// Returns metadata available before creating a session.
    fn definition() -> Definition;

    /// Initializes private policy state for the selected seat and recorded seed.
    async fn init(context: InitContext<Self::Config>) -> Result<Self>;

    /// Receives the first role-private view after successful initialization.
    async fn start(&mut self, _view: Self::View) -> Result<()> {
        Ok(())
    }

    /// Receives accepted actions and player messages after the initial view.
    async fn observe(&mut self, _event: Observation<Self::Action, Self::View>) -> Result<()> {
        Ok(())
    }

    /// Produces at most one action, message, and wakeup request.
    async fn respond(&mut self, reason: RespondReason) -> Result<Response<Self::Action>>;

    /// Receives the game's completion data when play reaches a terminal frame.
    async fn finish(&mut self, _completion: Self::Completion) -> Result<()> {
        Ok(())
    }

    /// Releases any session resources once the host is done with this policy.
    async fn shutdown(&mut self) -> Result<()> {
        Ok(())
    }
}

/// Exports a typed [`Agent`] as a component.
///
/// Call once at the crate root. Build as a `cdylib` for
/// `wasm32-unknown-unknown`, then use `wasm-tools component new`. On native
/// targets this checks the trait bound and leaves the policy available to tests.
#[macro_export]
macro_rules! export_agent {
    ($agent:ty $(,)?) => {
        const _: fn() = {
            fn check<T: $crate::agent::Agent>() {}
            check::<$agent>
        };

        #[cfg(target_arch = "wasm32")]
        type __ParlandoAgent = $agent;

        #[cfg(target_arch = "wasm32")]
        mod __parlando_agent_component {
            use super::__ParlandoAgent as Agent;
            use $crate::agent::__bindings::exports::parlando::agent::policy::{
                Guest, Observation, RespondReason, Response, Role,
            };
            use $crate::agent::component::Session;

            ::std::thread_local! {
                /// Stores the single policy instance owned by this component.
                static SESSION: ::std::cell::RefCell<Session<Agent>> =
                    const { ::std::cell::RefCell::new(Session::new()) };
            }

            /// Connects generated exports to the typed session.
            struct Component;

            impl Guest for Component {
                /// Provides static discovery data before any session starts.
                fn describe() -> Result<String, String> { Session::<Agent>::describe() }

                /// Constructs the policy using the selected role and seed.
                fn init(config: Vec<u8>, role: Role, seed: u64) -> Result<(), String> {
                    SESSION.with_borrow_mut(|session| session.init(&config, role, seed))
                }

                /// Delivers the initial observation.
                fn start(view: Vec<u8>) -> Result<(), String> {
                    SESSION.with_borrow_mut(|session| session.start(view))
                }

                /// Delivers the game's accepted events.
                fn observe(event: Observation) -> Result<(), String> {
                    SESSION.with_borrow_mut(|session| session.observe(event))
                }

                /// Requests a policy decision for the designated reason.
                fn respond(reason: RespondReason) -> Result<Response, String> {
                    SESSION.with_borrow_mut(|session| session.respond(reason))
                }

                /// Delivers terminal game data.
                fn finish(completion: Vec<u8>) -> Result<(), String> {
                    SESSION.with_borrow_mut(|session| session.finish(&completion))
                }

                /// Releases this component's policy instance.
                fn shutdown() -> Result<(), String> {
                    SESSION.with_borrow_mut(Session::shutdown)
                }
            }

            $crate::agent::__bindings::export!(Component with_types_in $crate::agent::__bindings);
        }
    };
}
