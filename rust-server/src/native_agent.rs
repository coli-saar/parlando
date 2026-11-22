//! Runs the same typed policy used by a component directly on the host runtime.

use std::{marker::PhantomData, sync::Arc};

use anyhow::{ensure, Context, Result};
use async_trait::async_trait;
use parlando_game_sdk::agent::{self as sdk, AgentView, ViewSource};
use parlando_game_sdk::{decode_declared, encode_compact, DecodeGameValue, GameValue};
use serde_json::Value;

use crate::{
    agents::{Agent, AgentContext, AgentFactory, AgentIdentity, Decision, Observation, Reason},
    component::{ComponentValue, LoadedGame, Schema},
    game::AgentDefinition,
    PlayerRole,
};

/// Shared game payload declarations used by both native and component adapters.
#[derive(Clone)]
pub(crate) struct PolicyLayouts {
    pub(crate) action: Arc<Schema>,
    view: [Option<Arc<Schema>>; 2],
    pub(crate) completion: Arc<Schema>,
}

impl PolicyLayouts {
    /// Checks the policy layouts and reuses the game's schema pointers for hot-path borrowing.
    pub(crate) fn for_game(
        game: &LoadedGame,
        action: &Schema,
        view: &Schema,
        completion: &Schema,
    ) -> Result<Self> {
        let game = game.schema();
        ensure!(
            game.action.as_ref() == action,
            "agent action layout differs from the game"
        );
        ensure!(
            game.completion.as_ref() == completion,
            "agent completion layout differs from the game"
        );
        let view = [&game.view_a, &game.view_b]
            .map(|layout| (layout.as_ref() == view).then(|| Arc::clone(layout)));
        ensure!(
            view.iter().any(Option::is_some),
            "agent view layout differs from both game roles"
        );
        Ok(Self {
            action: Arc::clone(&game.action),
            view,
            completion: Arc::clone(&game.completion),
        })
    }

    /// Returns the supported layout for this seat or explains the incompatibility.
    pub(crate) fn view(&self, role: PlayerRole) -> Result<&Arc<Schema>> {
        let index = match role {
            PlayerRole::A => 0,
            PlayerRole::B => 1,
        };
        self.view[index]
            .as_ref()
            .with_context(|| format!("agent view layout differs from game role {}", role.as_str()))
    }
}

/// Registers a typed Rust agent in either the live or headless runner.
///
/// Use [`Self::for_game`] when the game component is available to resolve
/// matching compact payloads at registration. Each session gets a new policy
/// and the host awaits its callbacks on the same asynchronous runner as other
/// agents.
pub struct TypedAgentFactory<A> {
    definition: AgentDefinition,
    identity: AgentIdentity,
    layouts: PolicyLayouts,
    agent: PhantomData<fn() -> A>,
}

impl<A: sdk::Agent> TypedAgentFactory<A> {
    /// Uses the policy's declared wire layouts when registration has no game handle.
    pub fn new() -> Result<Self> {
        let view = Arc::new(A::View::view_schema());
        Self::with_layouts(PolicyLayouts {
            action: Arc::new(A::Action::schema()),
            view: [Some(view.clone()), Some(view)],
            completion: Arc::new(A::Completion::schema()),
        })
    }

    /// Registers a policy against the selected game and checks seat compatibility up front.
    pub fn for_game(game: &LoadedGame) -> Result<Self> {
        Self::with_layouts(PolicyLayouts::for_game(
            game,
            &A::Action::schema(),
            &A::View::view_schema(),
            &A::Completion::schema(),
        )?)
    }

    /// Validates the same registration definition exposed by the component model.
    fn with_layouts(layouts: PolicyLayouts) -> Result<Self> {
        let authored = A::definition();
        let definition = AgentDefinition {
            id: authored.id.into(),
            name: authored.name.into(),
            description: authored.description.into(),
            config_fields: serde_json::from_str(authored.config_fields)
                .context("reading agent configuration fields")?,
        };
        definition.validate()?;
        let identity = AgentIdentity {
            name: authored.name.into(),
            version: authored.version.into(),
        };
        identity.validate()?;
        Ok(Self {
            definition,
            identity,
            layouts,
            agent: PhantomData,
        })
    }
}

#[async_trait]
impl<A: sdk::Agent> AgentFactory for TypedAgentFactory<A> {
    /// Returns the policy's stable dashboard definition.
    fn definition(&self) -> AgentDefinition {
        self.definition.clone()
    }

    /// Initializes one policy by awaiting its typed initialization callback.
    async fn create(&self, context: AgentContext) -> Result<Box<dyn Agent + Send>> {
        ensure!(
            context.factory_secrets.is_empty() && context.agent_instance_secrets.is_empty(),
            "typed agents do not receive secrets"
        );
        let view = Arc::clone(self.layouts.view(context.role)?);
        let policy = A::init(sdk::InitContext {
            config: serde_json::from_value(context.settings)?,
            role: context.role,
            seed: context.seed,
        })
        .await?;
        Ok(Box::new(TypedAgent {
            policy,
            action: Arc::clone(&self.layouts.action),
            view,
            completion: Arc::clone(&self.layouts.completion),
        }))
    }

    /// Returns the policy's declared implementation release.
    fn identity(&self, _settings: &Value) -> Result<AgentIdentity> {
        Ok(self.identity.clone())
    }
}

/// Owns one native policy and interprets payloads using its checked layouts.
struct TypedAgent<A> {
    policy: A,
    action: Arc<Schema>,
    view: Arc<Schema>,
    completion: Arc<Schema>,
}

#[async_trait]
impl<A: sdk::Agent> Agent for TypedAgent<A> {
    /// Decodes and delivers the first complete view.
    async fn start(&mut self, view: ComponentValue) -> Result<()> {
        let view = A::View::receive(ViewSource::from_component(view, &self.view)?)?;
        self.policy.start(view).await
    }

    /// Delivers the game transition and optional player message in typed form.
    async fn observe(&mut self, observation: Observation) -> Result<()> {
        let transition = observation
            .transition
            .map(|transition| {
                Ok::<_, anyhow::Error>(sdk::Transition {
                    actor: transition.actor,
                    action: decode(&transition.action, &self.action)?,
                    view: A::View::receive(ViewSource::from_component(
                        transition.view,
                        &self.view,
                    )?)?,
                })
            })
            .transpose()?;
        self.policy
            .observe(sdk::Observation {
                transition,
                message: observation.message,
            })
            .await
    }

    /// Awaits a decision and encodes an action directly into its compact layout.
    async fn decide(&mut self, reason: Reason) -> Result<Decision> {
        let outcome = self.policy.respond(reason).await?;
        let action = outcome
            .action
            .map(|action| {
                let bytes = encode_compact(&action).map_err(anyhow::Error::msg)?;
                ComponentValue::from_schema_bytes(bytes, Arc::clone(&self.action))
            })
            .transpose()?;
        Ok(Decision {
            action,
            message: outcome.message,
            wakeup: outcome.wakeup,
        })
    }

    /// Decodes and delivers the shared terminal value.
    async fn finish(&mut self, completion: ComponentValue) -> Result<()> {
        self.policy
            .finish(decode(&completion, &self.completion)?)
            .await
    }

    /// Waits for the policy's cleanup callback.
    async fn shutdown(&mut self) -> Result<()> {
        self.policy.shutdown().await
    }
}

/// Borrows a compatible wire buffer and decodes directly into the requested policy type.
fn decode<T: for<'de> DecodeGameValue<'de>>(value: &ComponentValue, schema: &Schema) -> Result<T> {
    decode_declared(&value.to_schema_messagepack(schema)?).map_err(anyhow::Error::msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SecretValues, SessionLogger};

    #[derive(serde::Deserialize)]
    struct Settings {
        action: u8,
    }

    struct WaitingPolicy(u8);

    #[sdk::async_trait]
    impl sdk::Agent for WaitingPolicy {
        type Config = Settings;
        type Action = u8;
        type View = u8;
        type Completion = u8;

        /// Supplies test metadata without affecting the policy's behavior.
        fn definition() -> sdk::Definition {
            sdk::Definition {
                id: "waiting",
                name: "Waiting",
                version: "1",
                description: "Exercises asynchronous native callbacks.",
                config_fields: "[]",
            }
        }

        /// Suspends before recording the configured action.
        async fn init(context: sdk::InitContext<Settings>) -> Result<Self> {
            tokio::task::yield_now().await;
            Ok(Self(context.config.action))
        }

        /// Suspends before combining the configured action and initial view.
        async fn start(&mut self, view: u8) -> Result<()> {
            tokio::task::yield_now().await;
            self.0 += view;
            Ok(())
        }

        /// Suspends before returning the policy's next action.
        async fn respond(&mut self, _reason: sdk::RespondReason) -> Result<sdk::Response<u8>> {
            tokio::task::yield_now().await;
            Ok(sdk::Response {
                action: Some(self.0),
                ..sdk::Response::default()
            })
        }
    }

    /// Keeps genuinely asynchronous callbacks available to direct native agents.
    #[tokio::test]
    async fn awaits_native_callbacks() -> Result<()> {
        let factory = TypedAgentFactory::<WaitingPolicy>::new()?;
        let mut agent = factory
            .create(AgentContext {
                role: PlayerRole::A,
                seed: 17,
                settings: serde_json::json!({"action": 3}),
                factory_secrets: SecretValues::default(),
                agent_instance_secrets: SecretValues::default(),
                logger: SessionLogger::headless(),
            })
            .await?;
        agent.start(ComponentValue::from_typed(&2_u8)?).await?;
        let decision = agent.decide(Reason::SessionStarted).await?;
        assert_eq!(decision.action.unwrap().decode::<u8>()?, 5);
        Ok(())
    }
}
