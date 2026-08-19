use std::env;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context as _, Result};
use async_trait::async_trait;
use parlando::{
    agent::{
        Agent, ConfigField as AgentConfigField, Context as AgentContext,
        Definition as AgentDefinition, Factory as AgentFactory, Identity as AgentIdentity,
        Response,
    },
    PlayerRole,
};
use serde_json::{json, Value};

use crate::game::ids::RootId;
use crate::game::types::{GreatTreeObservation, RootView};
use crate::{Action, GreatTree};

/// An agent that connects and does nothing: it never proposes an action or sends a message.
/// Useful for exercising pairing, observation delivery, and the participant client against a
/// live second player without needing a real playing agent yet.
pub struct IdleAgent;

#[async_trait]
impl Agent<GreatTree> for IdleAgent {
    async fn respond(&mut self, _available_actions: Option<Vec<Action>>) -> Result<Option<Response<Action>>> {
        Ok(None)
    }
}

pub struct IdleAgentFactory;

#[async_trait]
impl AgentFactory<GreatTree> for IdleAgentFactory {
    fn definition(&self) -> AgentDefinition {
        AgentDefinition {
            id: "great_tree.idle".to_string(),
            name: "Idle".to_string(),
            description: "Connects and takes no actions or messages.".to_string(),
            config_fields: Vec::new(),
        }
    }

    async fn create(&self, _context: AgentContext) -> Result<Box<dyn Agent<GreatTree> + Send>> {
        Ok(Box::new(IdleAgent))
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
    latest_roots: Option<[RootView; 5]>,
    pending_action: Option<Action>,
    pending_message: Option<String>,
}

impl RootBotAgent {
    fn new() -> Self {
        Self { latest_roots: None, pending_action: None, pending_message: None }
    }

    /// Opens the first thawed-but-not-running root found, queuing both the action and a status
    /// message for the next `respond`. A no-op if nothing needs opening.
    fn react(&mut self, roots: &[RootView; 5]) {
        self.latest_roots = Some(*roots);
        for view in roots {
            if view.thawed && !view.running {
                self.pending_action = Some(Action::SetFlow { root: view.id, open: true });
                self.pending_message = Some(format!("Opened {:?} — it's running now.", view.id));
                return;
            }
        }
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
            if words.iter().any(|w| *w == name) {
                let reply = match self.latest_roots.and_then(|roots| {
                    roots.iter().find(|view| view.id == root).copied()
                }) {
                    Some(view) if view.running => {
                        format!("{root:?} is thawed and I've got it open.")
                    }
                    Some(_) => format!("{root:?} isn't thawed yet."),
                    None => format!("I don't have a reading on {root:?} yet."),
                };
                self.pending_message = Some(reply);
                return;
            }
        }

        if words.iter().any(|w| w == "help" || w == "stuck") || text.contains('?') {
            self.pending_message = Some(ROOT_BOT_ORIENTATION.to_string());
            return;
        }

        if words.iter().any(|w| w == "hi" || w == "hello" || w == "hey") {
            self.pending_message = Some("Hey! Ready when you are.".to_string());
        }
    }
}

#[async_trait]
impl Agent<GreatTree> for RootBotAgent {
    async fn start(&mut self, initial_observation: GreatTreeObservation) -> Result<()> {
        self.pending_message = Some(ROOT_BOT_ORIENTATION.to_string());
        if let GreatTreeObservation::Root { roots } = initial_observation {
            self.react(&roots);
        }
        Ok(())
    }

    async fn observe_transition(
        &mut self,
        _actor: PlayerRole,
        _action: Action,
        observation: GreatTreeObservation,
    ) -> Result<()> {
        if let GreatTreeObservation::Root { roots } = observation {
            self.react(&roots);
        }
        Ok(())
    }

    async fn observe_message(&mut self, _sender: PlayerRole, text: String) -> Result<()> {
        self.handle_message(&text);
        Ok(())
    }

    async fn respond(
        &mut self,
        _available_actions: Option<Vec<Action>>,
    ) -> Result<Option<Response<Action>>> {
        let action = self.pending_action.take();
        let message = self.pending_message.take();
        Ok(match (action, message) {
            (Some(action), Some(message)) => Some(Response::action_and_message(action, message)),
            (Some(action), None) => Some(Response::action(action)),
            (None, Some(message)) => Some(Response::message(message)),
            (None, None) => None,
        })
    }
}

pub struct RootBotAgentFactory;

#[async_trait]
impl AgentFactory<GreatTree> for RootBotAgentFactory {
    fn definition(&self) -> AgentDefinition {
        AgentDefinition {
            id: "great_tree.root_bot".to_string(),
            name: "Root Bot (no LLM)".to_string(),
            description: "Scripted, non-LLM agent that plays Root: it opens any root that \
                warms up and keeps it open, so a human Crown can win by holding sun on any \
                three limbs at once. Sends a few canned chat lines but never reads the \
                partner's messages for its decisions. Requires no API key."
                .to_string(),
            config_fields: Vec::new(),
        }
    }

    async fn create(&self, _context: AgentContext) -> Result<Box<dyn Agent<GreatTree> + Send>> {
        Ok(Box::new(RootBotAgent::new()))
    }
}

/// Which side of the game this agent turned out to be controlling, learned from the `role` tag
/// on the first observation delivered to `start` — never known before that, since `AgentContext`
/// only carries a seat (`PlayerRole::A`/`B`), not which seat is Crown for this session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GameRole {
    Crown,
    Root,
}

/// LLM-backed agent that plays either role without being told this game's rules. It receives
/// exactly the same observation schema a human's client would drive its rendering from (field
/// names like `sun`/`water`/`thawed`/`running`, item names like `spire`/`hand`) — that much is
/// unavoidable for a text-only player — but it is never told the hold cap, the FIFO eviction,
/// the frozen/thaw relationship, the win condition, or that a hidden limb–root pairing exists.
/// It has to work all of that out the same way a human does: by acting, watching what changes
/// (or doesn't — a rejected action produces no observation at all, exactly like a human clicking
/// an unresponsive gate), and talking with its partner.
pub struct LlmAgent {
    api_key: String,
    model: String,
    client: reqwest::Client,
    own_role: PlayerRole,
    game_role: Option<GameRole>,
    /// Full session transcript re-sent on every decision; sessions are short (5-10 minutes) so
    /// no summarization or trimming is needed.
    history: Vec<Value>,
    /// Set whenever the partner does something new; cleared once this agent has considered it.
    /// Own actions/messages echo back too (see `observe_transition`/`observe_message`) but must
    /// not re-arm this, or the agent would keep reacting to itself and crowd out its partner.
    should_consider: bool,
    last_call: Option<Instant>,
}

impl LlmAgent {
    fn new(own_role: PlayerRole, api_key: String, model: String) -> Self {
        Self {
            api_key,
            model,
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .build()
                .expect("reqwest client builds with a fixed timeout"),
            own_role,
            game_role: None,
            history: Vec::new(),
            should_consider: false,
            last_call: None,
        }
    }

    fn system_prompt(&self) -> String {
        "You are playing a cooperative two-player exploration game with a human partner, \
communicating over text chat. Neither of you has been told this game's rules. You must work \
them out together, by trying things, watching what changes, and comparing notes over chat.\n\n\
You control one side of the game; your side is named in the `role` field of every observation \
below. You can only ever see your own side in full detail — you have no visibility into your \
partner's side at all, except whatever they choose to tell you.\n\n\
You have a tool called `act` that changes one property on one of your five items, and a tool \
called `message` that sends your partner a line of chat. On any turn you may call `act`, \
`message`, both, or neither — there's no requirement to do something every turn, and \
sometimes the useful move is to wait and see what your partner does.\n\n\
Since neither of you knows the rules yet, treat everything you notice as a hypothesis, not a \
settled fact. Say things like \"I think X causes Y — want to test it?\" rather than asserting \
confidently. If you try something and nothing in your next observation changed, that's \
meaningful too: it likely means whatever you tried didn't work, for some reason you don't \
understand yet. Be a curious, attentive partner: react to what your partner reports, propose \
small experiments, and build a shared understanding together as you play. Keep messages short \
and conversational, the way one player would actually talk to another mid-game.\n\n\
Below is the complete history of the session so far, oldest first, as JSON: every observation \
you've received, every action either of you has taken (and its visible effect on your side), \
and every chat message either of you has sent. Decide what to do next."
            .to_string()
    }

    fn tool_schemas(&self) -> Value {
        let act_parameters = match self.game_role {
            Some(GameRole::Crown) => json!({
                "type": "object",
                "properties": {
                    "limb": {
                        "type": "string",
                        "enum": ["spire", "hook", "fork", "cradle", "nub"]
                    },
                    "lit": {"type": "boolean"}
                },
                "required": ["limb", "lit"],
                "additionalProperties": false
            }),
            Some(GameRole::Root) => json!({
                "type": "object",
                "properties": {
                    "root": {
                        "type": "string",
                        "enum": ["hand", "knot", "tip", "swollen", "deep"]
                    },
                    "open": {"type": "boolean"}
                },
                "required": ["root", "open"],
                "additionalProperties": false
            }),
            None => json!({"type": "object", "properties": {}}),
        };
        json!([
            {
                "type": "function",
                "function": {
                    "name": "act",
                    "description": "Change one property on one of your five items.",
                    "parameters": act_parameters
                }
            },
            {
                "type": "function",
                "function": {
                    "name": "message",
                    "description": "Send a chat message to your partner.",
                    "parameters": {
                        "type": "object",
                        "properties": {"text": {"type": "string"}},
                        "required": ["text"],
                        "additionalProperties": false
                    }
                }
            }
        ])
    }

    /// Builds the tagged `Action` JSON `apply_action` expects from an `act` tool call's
    /// role-specific arguments, then leans on `Action`'s own `Deserialize` to validate shape.
    fn action_from_args(&self, args: &Value) -> Result<Action> {
        let tagged = match self.game_role {
            Some(GameRole::Crown) => json!({
                "type": "setSun",
                "limb": args.get("limb").cloned().unwrap_or(Value::Null),
                "lit": args.get("lit").cloned().unwrap_or(Value::Null),
            }),
            Some(GameRole::Root) => json!({
                "type": "setFlow",
                "root": args.get("root").cloned().unwrap_or(Value::Null),
                "open": args.get("open").cloned().unwrap_or(Value::Null),
            }),
            None => anyhow::bail!("great_tree.llm agent: action requested before role was known"),
        };
        Ok(serde_json::from_value(tagged)?)
    }

    async fn call_openai(&self) -> Result<Value> {
        let body = json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": self.system_prompt()},
                {"role": "user", "content": serde_json::to_string(&self.history)?},
            ],
            "tools": self.tool_schemas(),
            "tool_choice": "auto",
        });
        let response = self
            .client
            .post("https://api.openai.com/v1/chat/completions")
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?;
        let status = response.status();
        let text = response.text().await?;
        if !status.is_success() {
            // Carries OpenAI's actual error body (e.g. "Incorrect API key provided") rather
            // than just the HTTP status, since that's what actually lets you tell a bad key
            // apart from a bad model name or a rate limit from the logged error alone.
            anyhow::bail!("OpenAI API returned {status}: {text}");
        }
        Ok(serde_json::from_str(&text)?)
    }

    /// Turns one OpenAI response into an optional agent response, recording this agent's own
    /// attempted action/message into the transcript immediately — an accepted action will also
    /// arrive later via `observe_transition`, but a rejected one never will (see the struct
    /// doc), so this is the only record this agent gets of having tried at all.
    fn build_response(&mut self, raw: &Value) -> Result<Option<Response<Action>>> {
        let message = raw
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("message"))
            .ok_or_else(|| anyhow!("OpenAI response had no choices[0].message"))?;

        let mut action: Option<Action> = None;
        let mut text: Option<String> = None;

        if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
            for call in calls {
                let function = call.get("function");
                let name = function
                    .and_then(|f| f.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let arguments: Value = function
                    .and_then(|f| f.get("arguments"))
                    .and_then(Value::as_str)
                    .and_then(|raw| serde_json::from_str(raw).ok())
                    .unwrap_or_else(|| json!({}));
                match name {
                    "act" if action.is_none() => {
                        if let Ok(parsed) = self.action_from_args(&arguments) {
                            action = Some(parsed);
                        }
                    }
                    "message" if text.is_none() => {
                        if let Some(t) = arguments.get("text").and_then(Value::as_str) {
                            text = Some(t.to_string());
                        }
                    }
                    _ => {}
                }
            }
        }
        if action.is_none() && text.is_none() {
            if let Some(content) = message.get("content").and_then(Value::as_str) {
                if !content.trim().is_empty() {
                    text = Some(content.trim().to_string());
                }
            }
        }

        if let Some(action) = &action {
            self.history.push(json!({"event": "you_attempted_action", "action": action}));
        }
        if let Some(text) = &text {
            self.history.push(json!({"event": "message", "from": "you", "text": text}));
        }

        Ok(match (action, text) {
            (Some(action), Some(text)) => Some(Response::action_and_message(action, text)),
            (Some(action), None) => Some(Response::action(action)),
            (None, Some(text)) => Some(Response::message(text)),
            (None, None) => None,
        })
    }
}

#[async_trait]
impl Agent<GreatTree> for LlmAgent {
    async fn start(&mut self, initial_observation: GreatTreeObservation) -> Result<()> {
        self.game_role = Some(match &initial_observation {
            GreatTreeObservation::Crown { .. } => GameRole::Crown,
            GreatTreeObservation::Root { .. } => GameRole::Root,
        });
        self.history.push(json!({"event": "start", "observation": initial_observation}));
        self.should_consider = true;
        Ok(())
    }

    async fn observe_transition(
        &mut self,
        actor: PlayerRole,
        action: Action,
        observation: GreatTreeObservation,
    ) -> Result<()> {
        let is_own = actor == self.own_role;
        self.history.push(json!({
            "event": "action",
            "actor": if is_own { "you" } else { "partner" },
            "action": action,
            "observation": observation,
        }));
        if !is_own {
            self.should_consider = true;
        }
        Ok(())
    }

    async fn observe_message(&mut self, sender: PlayerRole, text: String) -> Result<()> {
        // The runtime never echoes an agent's own sent messages back to it (unlike actions,
        // which do echo); this branch only guards against that changing later.
        let is_own = sender == self.own_role;
        if is_own {
            return Ok(());
        }
        self.history.push(json!({"event": "message", "from": "partner", "text": text}));
        self.should_consider = true;
        Ok(())
    }

    async fn respond(
        &mut self,
        _available_actions: Option<Vec<Action>>,
    ) -> Result<Option<Response<Action>>> {
        if !self.should_consider {
            return Ok(None);
        }
        if let Some(last) = self.last_call {
            if last.elapsed() < Duration::from_millis(500) {
                return Ok(None);
            }
        }
        self.should_consider = false;
        self.last_call = Some(Instant::now());

        let raw = match self.call_openai().await {
            Ok(raw) => raw,
            Err(error) => {
                // `error`, not `warn`: main.rs's default EnvFilter (no RUST_LOG set) only
                // surfaces ERROR and above, and a broken/rejected OpenAI call otherwise fails
                // completely silently from here (Ok(None) is indistinguishable from "chose not
                // to respond"), with nothing recorded in the dashboard's session log either.
                tracing::error!(%error, "great_tree.llm agent: OpenAI call failed, skipping this turn");
                return Ok(None);
            }
        };
        self.build_response(&raw)
    }
}

pub struct LlmAgentFactory;

#[async_trait]
impl AgentFactory<GreatTree> for LlmAgentFactory {
    fn definition(&self) -> AgentDefinition {
        AgentDefinition {
            id: "great_tree.llm".to_string(),
            name: "LLM (learns as it plays)".to_string(),
            description: "OpenAI-backed agent that is never told this game's rules; it plays \
                either role by experimenting and talking with its partner, the same way a human \
                would have to. Requires OPENAI_API_KEY in the server environment."
                .to_string(),
            config_fields: vec![AgentConfigField {
                key: "model".to_string(),
                label: "OpenAI model".to_string(),
                help: "Chat Completions model name used for this agent's decisions, e.g. \
                    gpt-4o-mini."
                    .to_string(),
                kind: "text".to_string(),
                required: false,
                default_value: Value::String("gpt-4o-mini".to_string()),
            }],
        }
    }

    async fn create(&self, context: AgentContext) -> Result<Box<dyn Agent<GreatTree> + Send>> {
        let api_key = env::var("OPENAI_API_KEY").context(
            "OPENAI_API_KEY must be set in the server environment to use the great_tree.llm agent",
        )?;
        let model = context
            .settings
            .get("model")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("gpt-4o-mini")
            .to_string();
        Ok(Box::new(LlmAgent::new(context.role, api_key, model)))
    }

    fn identity(&self, settings: &Value) -> Result<AgentIdentity> {
        let model = settings
            .get("model")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("gpt-4o-mini")
            .to_string();
        Ok(AgentIdentity {
            name: "LlmAgent".to_string(),
            version: Some(model),
        })
    }
}

#[cfg(test)]
mod root_bot_tests {
    use super::*;

    fn roots(thawed_running: [(bool, bool); 5]) -> [RootView; 5] {
        let mut views = [RootView { id: RootId::Hand, thawed: false, running: false }; 5];
        for (view, (id, (thawed, running))) in
            views.iter_mut().zip(RootId::ALL.into_iter().zip(thawed_running))
        {
            *view = RootView { id, thawed, running };
        }
        views
    }

    async fn take_response(agent: &mut RootBotAgent) -> Option<Response<Action>> {
        agent.respond(None).await.unwrap()
    }

    #[tokio::test]
    async fn opens_a_newly_thawed_root() {
        let mut agent = RootBotAgent::new();
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
                Action::SetFlow { root: RootId::Knot, open: true },
                "Opened Knot — it's running now.".to_string(),
            )
        );
    }

    #[tokio::test]
    async fn never_reopens_an_already_running_root() {
        let mut agent = RootBotAgent::new();
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
        let mut agent = RootBotAgent::new();
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
        let mut agent = RootBotAgent::new();
        agent.react(&roots([
            (false, false),
            (false, false),
            (true, true), // Tip: running
            (false, false),
            (false, false),
        ]));
        take_response(&mut agent).await; // drain the "Opened Tip" response first
        agent.handle_message("is tip running yet?");
        let response = take_response(&mut agent).await.expect("should reply about Tip");
        assert_eq!(
            response,
            Response::message("Tip is thawed and I've got it open.".to_string())
        );
    }

    #[tokio::test]
    async fn replies_to_a_help_request_with_the_orientation_line() {
        let mut agent = RootBotAgent::new();
        agent.handle_message("I'm stuck, what do I do?");
        assert_eq!(
            take_response(&mut agent).await,
            Some(Response::message(ROOT_BOT_ORIENTATION.to_string()))
        );
    }

    #[tokio::test]
    async fn replies_to_a_greeting() {
        let mut agent = RootBotAgent::new();
        agent.handle_message("hey there!");
        assert_eq!(
            take_response(&mut agent).await,
            Some(Response::message("Hey! Ready when you are.".to_string()))
        );
    }

    #[tokio::test]
    async fn ignores_a_message_with_no_matching_keyword() {
        let mut agent = RootBotAgent::new();
        agent.handle_message("nice weather today");
        assert_eq!(take_response(&mut agent).await, None);
    }
}
