//! Root Bot reacts to its own controls and answers questions about visible roots.
use crate::agent_response::Response;
use crate::game::types::{GreatTreeCompletion, GreatTreeObservation, RootView};
use crate::{Action, RootId};
use parlando_game_sdk::{agent as sdk, Role as PlayerRole};
use sdk::Result;

use crate::ScriptedConfig as Config;

#[sdk::async_trait]
impl sdk::Agent for RootBotAgent {
    type Config = Config;
    type View = GreatTreeObservation;
    type Action = Action;
    type Completion = GreatTreeCompletion;
    /// Identifies the agent in the deployment host.
    fn definition() -> sdk::Definition {
        sdk::Definition {
            id: "great_tree.root_bot",
            name: "Root Bot (no LLM)",
            description: "Opens thawed roots and reports their state to its partner.",
            version: "2",
            config_fields: "[]",
        }
    }
    /// Initializes the component with no external logging sink.
    async fn init(_: sdk::InitContext<Config>) -> Result<Self> {
        Ok(Self::new(None))
    }
    /// Delivers the initial view to the Root player's strategy.
    async fn start(&mut self, view: Self::View) -> Result<()> {
        Self::start(self, view).await
    }
    /// Updates the state before handling a message delivered with an action.
    async fn observe(&mut self, event: sdk::Observation<Action, Self::View>) -> Result<()> {
        if let Some(transition) = event.transition {
            self.observe_transition(transition.actor, transition.action, transition.view)
                .await?;
        }
        if let Some(message) = event.message {
            self.observe_message(message.sender, message.text).await?;
        }
        Ok(())
    }
    /// Returns the queued action and chat for this decision.
    async fn respond(&mut self, _: sdk::RespondReason) -> Result<sdk::Response<Action>> {
        Ok(Self::respond(self)
            .await?
            .map(sdk::Response::from)
            .unwrap_or_default())
    }
}

const ROOT_BOT_ORIENTATION: &str =
    "I'll open any root that warms up on my side, and keep it open once it's flowing.";

/// Non-LLM agent that plays Root by reacting only to its own observation. Its entire policy:
/// open any root that is thawed and not already running, and never voluntarily close one.
///
/// This is always safe against Root's own 3-hold cap without any eviction logic: Crown can
/// hold sun on at most 3 limbs at once (Crown's own cap), and the hidden bijection is one limb
/// per root, so at most 3 roots are ever thawed simultaneously — exactly Root's cap. So a human
/// Crown can win outright just by holding sun on any 3 limbs at once and waiting.
///
/// It never interprets the partner's chat for decisions — `handle_message` only replies with
/// canned lines triggered by simple keyword matching, purely for the human's benefit.
pub struct RootBotAgent {
    logger: Option<Box<dyn Fn(String) + Send + Sync>>,
    latest_roots: Option<[RootView; 5]>,
    pending_action: Option<Action>,
    pending_message: Option<String>,
}

impl RootBotAgent {
    /// Creates one session-owned RootBot with its runtime-attributed logger.
    pub(super) fn new(logger: Option<Box<dyn Fn(String) + Send + Sync>>) -> Self {
        Self {
            logger,
            latest_roots: None,
            pending_action: None,
            pending_message: None,
        }
    }

    /// Creates an isolated RootBot for mechanics tests.
    #[cfg(test)]
    fn testing() -> Self {
        Self::new(None)
    }

    /// Records an observational message without letting logging affect play.
    fn log(&self, message: impl Into<String>) {
        if let Some(logger) = &self.logger {
            logger(message.into());
        }
    }

    /// Opens the first thawed-but-not-running root found, queuing both the action and a status
    /// message for the next `respond`. A no-op if nothing needs opening.
    fn react(&mut self, roots: &[RootView; 5]) {
        self.latest_roots = Some(*roots);
        for view in roots {
            if view.thawed && !view.running {
                let action = Action::SetFlow {
                    root: view.id,
                    open: true,
                };
                self.log(format!("RootBot queued action: {action:?}"));
                self.pending_action = Some(action);
                self.pending_message = Some(format!("Opened {:?} — it's running now.", view.id));
                return;
            }
        }
        self.log("RootBot observed no thawed root requiring an action");
    }

    /// Replies to the partner's chat using plain keyword matching — never full comprehension.
    /// Checked in order: naming a root reports that root's current status; "help"/"stuck"/"?"
    /// repeats the orientation line; a greeting gets a friendly reply. No match, no reply.
    fn handle_message(&mut self, text: &str) {
        let words: Vec<String> = text
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(|w| w.to_lowercase())
            .collect();

        for &root in RootId::ALL.iter() {
            let name = format!("{root:?}").to_lowercase();
            if words.contains(&name) {
                let reply = match self
                    .latest_roots
                    .and_then(|roots| roots.iter().find(|view| view.id == root).copied())
                {
                    Some(view) if view.running => {
                        format!("{root:?} is thawed and I've got it open.")
                    }
                    Some(_) => format!("{root:?} isn't thawed yet."),
                    None => format!("I don't have a reading on {root:?} yet."),
                };
                self.log(format!("RootBot queued status reply for {root:?}"));
                self.pending_message = Some(reply);
                return;
            }
        }

        if words.iter().any(|w| w == "help" || w == "stuck") || text.contains('?') {
            self.log("RootBot queued its orientation reply");
            self.pending_message = Some(ROOT_BOT_ORIENTATION.to_string());
            return;
        }

        if words
            .iter()
            .any(|w| w == "hi" || w == "hello" || w == "hey")
        {
            self.log("RootBot queued a greeting reply");
            self.pending_message = Some("Hey! Ready when you are.".to_string());
        }
    }
}

impl RootBotAgent {
    /// Queues the opening remarks and observes the initial controls.
    pub(super) async fn start(&mut self, initial_observation: GreatTreeObservation) -> Result<()> {
        self.log("RootBot started and queued its orientation message");
        self.pending_message = Some(ROOT_BOT_ORIENTATION.to_string());
        if let GreatTreeObservation::Root { roots } = initial_observation {
            self.react(&roots);
        } else {
            self.log("RootBot received a Crown observation and cannot apply its Root policy");
        }
        Ok(())
    }

    /// Applies the latest private root view after an accepted move.
    pub(super) async fn observe_transition(
        &mut self,
        actor: PlayerRole,
        action: Action,
        observation: GreatTreeObservation,
    ) -> Result<()> {
        self.log(format!(
            "RootBot observed accepted action from role {}: {action:?}",
            actor.as_str()
        ));
        if let GreatTreeObservation::Root { roots } = observation {
            self.react(&roots);
        }
        Ok(())
    }

    /// Answers participant chat using observed root conditions.
    pub(super) async fn observe_message(&mut self, sender: PlayerRole, text: String) -> Result<()> {
        self.log(format!(
            "RootBot received message from role {}: {text:?}",
            sender.as_str()
        ));
        self.handle_message(&text);
        Ok(())
    }

    /// Drains the pending action and message for a single decision.
    pub(super) async fn respond(&mut self) -> Result<Option<Response<Action>>> {
        let action = self.pending_action.take();
        let message = self.pending_message.take();
        self.log(format!(
            "RootBot response: action={}, message={}",
            action.is_some(),
            message.is_some()
        ));
        Ok(match (action, message) {
            (Some(action), Some(message)) => Some(Response::action_and_message(action, message)),
            (Some(action), None) => Some(Response::action(action)),
            (None, Some(message)) => Some(Response::message(message)),
            (None, None) => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn roots(thawed_running: [(bool, bool); 5]) -> [RootView; 5] {
        let mut views = [RootView {
            id: RootId::Hand,
            thawed: false,
            running: false,
        }; 5];
        for (view, (id, (thawed, running))) in views
            .iter_mut()
            .zip(RootId::ALL.into_iter().zip(thawed_running))
        {
            *view = RootView {
                id,
                thawed,
                running,
            };
        }
        views
    }

    async fn take_response(agent: &mut RootBotAgent) -> Option<Response<Action>> {
        agent.respond().await.unwrap()
    }

    #[tokio::test]
    async fn opens_a_newly_thawed_root() {
        let mut agent = RootBotAgent::testing();
        agent.react(&roots([
            (false, false),
            (true, false), // Knot: thawed, not yet running
            (false, false),
            (false, false),
            (false, false),
        ]));
        let response = take_response(&mut agent).await.expect("should open Knot");
        assert_eq!(
            response,
            Response::action_and_message(
                Action::SetFlow {
                    root: RootId::Knot,
                    open: true
                },
                "Opened Knot — it's running now.".to_string(),
            )
        );
    }

    #[tokio::test]
    async fn never_reopens_an_already_running_root() {
        let mut agent = RootBotAgent::testing();
        agent.react(&roots([
            (true, true), // Hand: thawed and already running
            (false, false),
            (false, false),
            (false, false),
            (false, false),
        ]));
        assert_eq!(take_response(&mut agent).await, None);
    }

    #[tokio::test]
    async fn never_closes_a_root_on_its_own() {
        let mut agent = RootBotAgent::testing();
        agent.react(&roots([
            (false, true), // Tip: running but no longer thawed
            (false, false),
            (false, false),
            (false, false),
            (false, false),
        ]));
        assert_eq!(
            take_response(&mut agent).await,
            None,
            "must never propose closing a root itself"
        );
    }

    #[tokio::test]
    async fn reports_a_named_roots_status_without_acting_on_it() {
        let mut agent = RootBotAgent::testing();
        agent.react(&roots([
            (false, false),
            (false, false),
            (true, true), // Tip: running
            (false, false),
            (false, false),
        ]));
        take_response(&mut agent).await; // drain the "Opened Tip" response first
        agent.handle_message("is tip running yet?");
        let response = take_response(&mut agent)
            .await
            .expect("should reply about Tip");
        assert_eq!(
            response,
            Response::message("Tip is thawed and I've got it open.".to_string())
        );
    }

    #[tokio::test]
    async fn replies_to_a_help_request_with_the_orientation_line() {
        let mut agent = RootBotAgent::testing();
        agent.handle_message("I'm stuck, what do I do?");
        assert_eq!(
            take_response(&mut agent).await,
            Some(Response::message(ROOT_BOT_ORIENTATION.to_string()))
        );
    }

    #[tokio::test]
    async fn replies_to_a_greeting() {
        let mut agent = RootBotAgent::testing();
        agent.handle_message("hey there!");
        assert_eq!(
            take_response(&mut agent).await,
            Some(Response::message("Hey! Ready when you are.".to_string()))
        );
    }

    #[tokio::test]
    async fn ignores_a_message_with_no_matching_keyword() {
        let mut agent = RootBotAgent::testing();
        agent.handle_message("nice weather today");
        assert_eq!(take_response(&mut agent).await, None);
    }
}
