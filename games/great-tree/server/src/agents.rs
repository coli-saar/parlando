use crate::agent_response::Response;
use parlando::component::ComponentValue;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context as _, Result};
use async_trait::async_trait;
use parlando::{
    agent::{
        Agent, ConfigField as AgentConfigField, Context as AgentContext,
        Definition as AgentDefinition, Factory as AgentFactory, Identity as AgentIdentity,
    },
    PlayerRole,
};
use serde_json::{json, Value};

use crate::game::types::GreatTreeObservation;
use crate::root_bot::RootBotAgent;
use crate::Action;

#[async_trait]
impl Agent for RootBotAgent {
    /// Delivers the decoded starting view to the policy.
    async fn start(&mut self, view: ComponentValue) -> Result<()> {
        Self::start(self, view.decode()?).await
    }

    /// Delivers the accepted action and this seat's decoded view.
    async fn observe_transition(
        &mut self,
        actor: PlayerRole,
        action: ComponentValue,
        view: ComponentValue,
    ) -> Result<()> {
        Self::observe_transition(self, actor, action.decode()?, view.decode()?).await
    }

    /// Delivers chat without modifying game state.
    async fn observe_message(&mut self, actor: PlayerRole, text: String) -> Result<()> {
        Self::observe_message(self, actor, text).await
    }

    /// Encodes a typed policy decision for the shared component host.
    async fn respond(&mut self) -> Result<Option<parlando::agent::Response>> {
        Self::respond(self)
            .await?
            .map(Response::into_wire)
            .transpose()
    }
}

pub struct RootBotAgentFactory;

#[async_trait]
impl AgentFactory for RootBotAgentFactory {
    fn definition(&self) -> AgentDefinition {
        let definition = <RootBotAgent as parlando_game_sdk::agent::Agent>::definition();
        AgentDefinition {
            id: definition.id.into(),
            name: definition.name.into(),
            description: definition.description.into(),
            config_fields: Vec::new(),
        }
    }

    async fn create(&self, context: AgentContext) -> Result<Box<dyn Agent + Send>> {
        let _ = context.logger.log(format!(
            "RootBot constructed for role {} with seed {}",
            context.role.as_str(),
            context.seed
        ));
        Ok(Box::new(RootBotAgent::new(Some(Box::new(move |text| {
            let _ = context.logger.log(text);
        })))))
    }

    fn identity(&self, _settings: &Value) -> Result<AgentIdentity> {
        Ok(AgentIdentity {
            name: "RootBotAgent".to_string(),
            version: "2".to_string(),
        })
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
            self.history
                .push(json!({"event": "you_attempted_action", "action": action}));
        }
        if let Some(text) = &text {
            self.history
                .push(json!({"event": "message", "from": "you", "text": text}));
        }

        Ok(match (action, text) {
            (Some(action), Some(text)) => Some(Response::action_and_message(action, text)),
            (Some(action), None) => Some(Response::action(action)),
            (None, Some(text)) => Some(Response::message(text)),
            (None, None) => None,
        })
    }
}

impl LlmAgent {
    async fn start(&mut self, initial_observation: GreatTreeObservation) -> Result<()> {
        self.game_role = Some(match &initial_observation {
            GreatTreeObservation::Crown { .. } => GameRole::Crown,
            GreatTreeObservation::Root { .. } => GameRole::Root,
        });
        self.history
            .push(json!({"event": "start", "observation": initial_observation}));
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
        self.history
            .push(json!({"event": "message", "from": "partner", "text": text}));
        self.should_consider = true;
        Ok(())
    }

    async fn respond(&mut self) -> Result<Option<Response<Action>>> {
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
#[async_trait]
impl Agent for LlmAgent {
    /// Delivers the decoded starting view to the policy.
    async fn start(&mut self, view: ComponentValue) -> Result<()> {
        Self::start(self, view.decode()?).await
    }

    /// Delivers the accepted action and this seat's decoded view.
    async fn observe_transition(
        &mut self,
        actor: PlayerRole,
        action: ComponentValue,
        view: ComponentValue,
    ) -> Result<()> {
        Self::observe_transition(self, actor, action.decode()?, view.decode()?).await
    }

    /// Delivers chat without modifying game state.
    async fn observe_message(&mut self, actor: PlayerRole, text: String) -> Result<()> {
        Self::observe_message(self, actor, text).await
    }

    /// Encodes a typed policy decision for the shared component host.
    async fn respond(&mut self) -> Result<Option<parlando::agent::Response>> {
        Self::respond(self)
            .await?
            .map(Response::into_wire)
            .transpose()
    }
}

pub struct LlmAgentFactory;

#[async_trait]
impl AgentFactory for LlmAgentFactory {
    fn definition(&self) -> AgentDefinition {
        AgentDefinition {
            id: "great_tree.llm".to_string(),
            name: "LLM (learns as it plays)".to_string(),
            description: "OpenAI-backed agent that is never told this game's rules; it plays \
                either role by experimenting and talking with its partner, the same way a human \
                would have to. Requires a configured experiment secret reference."
                .to_string(),
            config_fields: vec![
                AgentConfigField {
                    key: "model".to_string(),
                    label: "OpenAI model".to_string(),
                    help: "Chat Completions model name used for this agent's decisions, e.g. \
                    gpt-4o-mini."
                        .to_string(),
                    value: parlando::agent::ConfigValue::String {
                        format: parlando::agent::StringFormat::Plain,
                    },
                    required: false,
                    default_value: Value::String("gpt-4o-mini".to_string()),
                },
                AgentConfigField {
                    key: "openai_api_key".to_string(),
                    label: "OpenAI API key".to_string(),
                    help: "Experiment secret used by the local agent factory.".to_string(),
                    value: parlando::agent::ConfigValue::SecretReference {
                        purpose: parlando::agent::SecretPurpose::Factory,
                    },
                    required: true,
                    default_value: Value::Null,
                },
            ],
        }
    }

    async fn create(&self, context: AgentContext) -> Result<Box<dyn Agent + Send>> {
        let api_key = context
            .factory_secrets
            .get("config.openai_api_key")
            .context("the configured OpenAI secret reference is unavailable")?
            .to_string();
        let model = context
            .settings
            .get("model")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("gpt-4o-mini")
            .to_string();
        Ok(Box::new(LlmAgent::new(context.role, api_key, model)))
    }

    fn identity(&self, _settings: &Value) -> Result<AgentIdentity> {
        Ok(AgentIdentity {
            name: "LlmAgent".to_string(),
            version: "1".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A model tool call produces the same typed action and chat a live seat submits.
    #[tokio::test]
    async fn model_decision_matches_the_game_interface() {
        use parlando_game_sdk::Game;
        let mut tree = crate::GreatTree::init(Default::default(), 7).unwrap();
        let mut agent = LlmAgent::new(PlayerRole::A, String::new(), "test".into());
        Agent::start(
            &mut agent,
            ComponentValue::from_typed(&tree.view_a()).unwrap(),
        )
        .await
        .unwrap();
        let reply = json!({"choices": [{"message": {"tool_calls": [
            {"function": {"name": "act", "arguments": r#"{"limb":"hook","lit":true}"#}},
            {"function": {"name": "message", "arguments": r#"{"text":"I lit Hook."}"#}}
        ]}}]});
        let decision = agent
            .build_response(&reply)
            .unwrap()
            .unwrap()
            .into_wire()
            .unwrap();
        let parlando::agent::Response::ActionAndMessage { action, message } = decision else {
            panic!("expected a move and its accompanying message");
        };
        tree.step(PlayerRole::A, action.decode().unwrap()).unwrap();
        assert_eq!(message, "I lit Hook.");
    }

    #[test]
    fn great_tree_agents_expose_their_current_identity_versions() {
        for identity in [
            crate::IdleAgentFactory::new()
                .unwrap()
                .identity(&json!({}))
                .unwrap(),
            LlmAgentFactory
                .identity(&json!({"model": "gpt-4o-mini"}))
                .unwrap(),
        ] {
            assert_eq!(identity.version, "1");
        }
        assert_eq!(
            RootBotAgentFactory.identity(&json!({})).unwrap().version,
            "2"
        );
    }
}
