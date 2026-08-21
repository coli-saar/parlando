use std::{
    collections::{BTreeMap, HashSet},
    fmt,
};

use anyhow::{bail, Context, Result};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;

use crate::SessionLogger;

/// Describes one configuration input accepted by a compiled agent factory.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct AgentConfigField {
    /// Stable object key written beneath the selected agent's configuration.
    pub key: String,
    /// Human-readable field label shown by the administrator dashboard.
    pub label: String,
    /// Brief explanation of the value's purpose.
    pub help: String,
    /// Semantic value accepted by this field. User interfaces derive controls from it.
    #[serde(flatten)]
    pub value: AgentConfigValue,
    /// Whether an administrator must supply a non-empty value.
    pub required: bool,
    /// Default value inserted when this factory is selected.
    pub default_value: Value,
}

/// Semantic format of a string-valued agent setting.
#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StringFormat {
    /// An unconstrained string.
    #[default]
    Plain,
    /// An absolute HTTP or HTTPS URI.
    Uri,
}

/// One stable stored choice and its human-readable label.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct AgentConfigChoice {
    /// Stable value persisted in experiment configuration.
    pub value: String,
    /// Label displayed to administrators.
    pub label: String,
}

/// Determines which consumer may receive a referenced secret value.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretPurpose {
    /// The local factory or transport adapter may consume the value.
    Factory,
    /// The constructed agent instance may consume the value.
    AgentInstance,
}

/// Semantic type of an agent configuration field.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentConfigValue {
    /// A string, optionally carrying a URI semantic constraint.
    String {
        #[serde(default)]
        format: StringFormat,
    },
    /// A Boolean value.
    Boolean,
    /// A signed integer with optional inclusive bounds.
    Integer {
        minimum: Option<i64>,
        maximum: Option<i64>,
    },
    /// A finite number with optional inclusive bounds.
    Number {
        minimum: Option<f64>,
        maximum: Option<f64>,
    },
    /// One value selected from a stable closed set.
    Choice { choices: Vec<AgentConfigChoice> },
    /// A recursively typed object.
    Object { fields: Vec<AgentConfigField> },
    /// The identifier of an experiment-owned game secret.
    SecretReference { purpose: SecretPurpose },
}

/// Secret values exposed to one trusted runtime consumer.
#[derive(Clone, Default)]
pub struct SecretValues(BTreeMap<String, String>);

impl SecretValues {
    /// Creates an isolated secret set keyed by semantic configuration path.
    pub(crate) fn new(values: BTreeMap<String, String>) -> Self {
        Self(values)
    }
    /// Looks up one explicitly delivered secret without permitting serialization.
    pub fn get(&self, path: &str) -> Option<&str> {
        self.0.get(path).map(String::as_str)
    }
    /// Returns whether this isolated set contains no values.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&String, &String)> {
        self.0.iter()
    }
}

impl fmt::Debug for SecretValues {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SecretValues")
            .field("values", &"[REDACTED]")
            .finish()
    }
}

/// Trusted inputs used to construct a game's initial state.
pub struct GameInitializationContext<'a, C> {
    /// Validated typed game configuration.
    pub config: &'a C,
    /// Recorded deterministic session seed.
    pub seed: u64,
    /// All experiment-owned game secrets, keyed without the `game.` prefix.
    pub secrets: &'a SecretValues,
}

/// Session-specific capability supplied while constructing one game value.
///
/// Unlike [`GameInitializationContext`], which exists only while the initial
/// authoritative state is produced, this context contains dependencies that a
/// game may retain and use throughout its session lifetime.
pub struct GameSessionContext {
    /// Logger already bound to this session and the game source.
    pub logger: SessionLogger,
}

/// Describes one agent implementation registered with a game server.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct AgentDefinition {
    /// Stable selector persisted in experiment configuration.
    pub id: String,
    /// Concise name shown in the administrator dashboard.
    pub name: String,
    /// Explanation of where the agent runs and how it behaves.
    pub description: String,
    /// Factory-specific structured configuration inputs.
    pub config_fields: Vec<AgentConfigField>,
}

impl AgentDefinition {
    /// Validates field keys, recursive definitions, defaults, and numeric constraints.
    pub fn validate(&self) -> Result<()> {
        if self.id.trim().is_empty() {
            bail!("agent definition id must not be empty");
        }
        validate_fields(&self.config_fields, "config")
    }

    /// Validates and normalizes stored settings, inserting declared defaults.
    pub fn normalize_settings(&self, settings: &Value) -> Result<Value> {
        if settings.is_null() && self.config_fields.is_empty() {
            return Ok(Value::Object(serde_json::Map::new()));
        }
        let object = settings
            .as_object()
            .context("agent settings must be an object")?;
        normalize_object(&self.config_fields, object, "config")
    }

    /// Resolves declared references into purpose-isolated secret sets.
    pub(crate) fn resolve_secrets(
        &self,
        settings: &Value,
        secrets: &std::collections::HashMap<String, String>,
    ) -> Result<(SecretValues, SecretValues)> {
        let mut factory = BTreeMap::new();
        let mut instance = BTreeMap::new();
        resolve_field_secrets(
            &self.config_fields,
            settings
                .as_object()
                .context("agent settings must be an object")?,
            "config",
            secrets,
            &mut factory,
            &mut instance,
        )?;
        Ok((SecretValues::new(factory), SecretValues::new(instance)))
    }
}

fn resolve_field_secrets(
    fields: &[AgentConfigField],
    values: &serde_json::Map<String, Value>,
    path: &str,
    secrets: &std::collections::HashMap<String, String>,
    factory: &mut BTreeMap<String, String>,
    instance: &mut BTreeMap<String, String>,
) -> Result<()> {
    for field in fields {
        let Some(value) = values.get(&field.key) else {
            continue;
        };
        let field_path = format!("{path}.{}", field.key);
        match &field.value {
            AgentConfigValue::SecretReference { purpose } => {
                let reference = value
                    .as_str()
                    .context("validated secret reference was not a string")?;
                let key = reference
                    .strip_prefix("game.")
                    .context("validated secret reference lost game prefix")?;
                let secret = secrets
                    .get(key)
                    .with_context(|| format!("missing experiment secret reference {reference}"))?
                    .clone();
                match purpose {
                    SecretPurpose::Factory => &mut *factory,
                    SecretPurpose::AgentInstance => &mut *instance,
                }
                .insert(field_path, secret);
            }
            AgentConfigValue::Object { fields } => resolve_field_secrets(
                fields,
                value
                    .as_object()
                    .context("validated object was not an object")?,
                &field_path,
                secrets,
                factory,
                instance,
            )?,
            _ => {}
        }
    }
    Ok(())
}

fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 128
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

fn validate_fields(fields: &[AgentConfigField], path: &str) -> Result<()> {
    let mut keys = HashSet::new();
    for field in fields {
        if !valid_key(&field.key) {
            bail!("invalid agent configuration key {path}.{}", field.key);
        }
        if !keys.insert(&field.key) {
            bail!("duplicate agent configuration key {path}.{}", field.key);
        }
        match &field.value {
            AgentConfigValue::Integer {
                minimum: Some(a),
                maximum: Some(b),
            } if a > b => bail!("impossible bounds for {path}.{}", field.key),
            AgentConfigValue::Number { minimum, maximum } => {
                if minimum.is_some_and(|v| !v.is_finite())
                    || maximum.is_some_and(|v| !v.is_finite())
                    || matches!((minimum, maximum), (Some(a), Some(b)) if a > b)
                {
                    bail!("invalid bounds for {path}.{}", field.key);
                }
            }
            AgentConfigValue::Choice { choices } => {
                let mut values = HashSet::new();
                if choices.is_empty()
                    || choices.iter().any(|c| {
                        c.value.is_empty() || c.label.trim().is_empty() || !values.insert(&c.value)
                    })
                {
                    bail!("invalid choices for {path}.{}", field.key);
                }
            }
            AgentConfigValue::Object { fields } => {
                validate_fields(fields, &format!("{path}.{}", field.key))?
            }
            _ => {}
        }
        if !field.default_value.is_null() {
            normalize_value(
                field,
                &field.default_value,
                &format!("{path}.{}", field.key),
            )
            .context("invalid agent configuration default")?;
        }
    }
    Ok(())
}

fn normalize_object(
    fields: &[AgentConfigField],
    object: &serde_json::Map<String, Value>,
    path: &str,
) -> Result<Value> {
    for key in object.keys() {
        if !fields.iter().any(|f| &f.key == key) {
            bail!("unknown agent setting {path}.{key}");
        }
    }
    let mut normalized = serde_json::Map::new();
    for field in fields {
        let value = object
            .get(&field.key)
            .or_else(|| (!field.default_value.is_null()).then_some(&field.default_value));
        match value {
            Some(value) => {
                normalized.insert(
                    field.key.clone(),
                    normalize_value(field, value, &format!("{path}.{}", field.key))?,
                );
            }
            None if field.required => {
                bail!("required agent setting {path}.{} is missing", field.key)
            }
            None => {}
        }
    }
    Ok(Value::Object(normalized))
}

fn normalize_value(field: &AgentConfigField, value: &Value, path: &str) -> Result<Value> {
    match &field.value {
        AgentConfigValue::String { format } => {
            let text = value
                .as_str()
                .with_context(|| format!("{path} must be a string"))?;
            if field.required && text.trim().is_empty() {
                bail!("{path} must not be empty");
            }
            if matches!(format, StringFormat::Uri) {
                let uri = text
                    .parse::<http::Uri>()
                    .with_context(|| format!("{path} must be a URI"))?;
                if !matches!(uri.scheme_str(), Some("http" | "https")) || uri.host().is_none() {
                    bail!("{path} must be an absolute HTTP or HTTPS URI");
                }
            }
            Ok(value.clone())
        }
        AgentConfigValue::Boolean => value
            .as_bool()
            .map(Value::Bool)
            .with_context(|| format!("{path} must be a boolean")),
        AgentConfigValue::Integer { minimum, maximum } => {
            let number = value
                .as_i64()
                .with_context(|| format!("{path} must be an integer"))?;
            if minimum.is_some_and(|v| number < v) || maximum.is_some_and(|v| number > v) {
                bail!("{path} is outside its bounds");
            }
            Ok(Value::from(number))
        }
        AgentConfigValue::Number { minimum, maximum } => {
            let number = value
                .as_f64()
                .filter(|v| v.is_finite())
                .with_context(|| format!("{path} must be a finite number"))?;
            if minimum.is_some_and(|v| number < v) || maximum.is_some_and(|v| number > v) {
                bail!("{path} is outside its bounds");
            }
            serde_json::Number::from_f64(number)
                .map(Value::Number)
                .context("finite number could not be normalized")
        }
        AgentConfigValue::Choice { choices } => {
            let selected = value
                .as_str()
                .with_context(|| format!("{path} must be a choice string"))?;
            if !choices.iter().any(|c| c.value == selected) {
                bail!("{path} is not a declared choice");
            }
            Ok(value.clone())
        }
        AgentConfigValue::Object { fields } => normalize_object(
            fields,
            value
                .as_object()
                .with_context(|| format!("{path} must be an object"))?,
            path,
        ),
        AgentConfigValue::SecretReference { .. } => {
            let reference = value
                .as_str()
                .with_context(|| format!("{path} must be a secret reference"))?;
            if !reference.starts_with("game.") || !valid_key(reference.trim_start_matches("game."))
            {
                bail!("{path} must reference a game.<key> secret");
            }
            Ok(value.clone())
        }
    }
}

/// Immutable identity of the game implementation compiled into a server process.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct GameMetadata {
    /// Stable machine-readable identifier shared by releases of this game.
    pub id: String,
    /// Human-readable game name displayed by administration tools.
    pub name: String,
    /// Exact semantic version used to decide whether an experiment can be activated.
    pub version: semver::Version,
    /// Diagnostic build provenance which does not affect activation compatibility.
    pub build_manifest: Value,
}

impl GameMetadata {
    /// Validates the stable process identity before the server opens its dashboard.
    pub(crate) fn validate(&self) -> Result<()> {
        if self.id.is_empty()
            || self.id.chars().count() > 128
            || !self
                .id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            anyhow::bail!(
                "game id must contain 1 to 128 letters, digits, dots, dashes, or underscores"
            );
        }
        if self.name.trim().is_empty() {
            anyhow::bail!("game name must not be empty");
        }
        Ok(())
    }
}

/// Identifies one of the two active player roles in a session.
///
/// Parlando deliberately models two-player games. Either role may be controlled
/// by a human participant or an agent; the role does not encode presentation or
/// participant kind.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum PlayerRole {
    /// The first active player role.
    A,
    /// The second active player role.
    B,
}

impl PlayerRole {
    /// Returns the stable wire-format role name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
        }
    }
}

/// Identifies a participant's runtime seat in a live session.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Seat {
    /// The first active player seat.
    #[serde(rename = "A")]
    A,
    /// The second active player seat.
    #[serde(rename = "B")]
    B,
}

impl Seat {
    /// Returns the stable wire-format seat name.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
        }
    }

    /// Converts the runtime seat into the public player role.
    pub(crate) fn player_role(self) -> PlayerRole {
        match self {
            Self::A => PlayerRole::A,
            Self::B => PlayerRole::B,
        }
    }
}

/// Machine-readable reason why a game rule rejected a typed action.
///
/// The code is part of the game-specific protocol. It must not contain rendered
/// prose or internal diagnostics: frontends may translate it, agents and logs may
/// inspect it, and the server treats it as an expected rejection rather than a
/// runtime failure.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ActionRejection {
    /// Stable game-specific reason code, such as `wrong_role`.
    pub code: String,
}

impl ActionRejection {
    /// Creates a rejection from one stable machine-readable reason code.
    pub fn new(code: impl Into<String>) -> Self {
        Self { code: code.into() }
    }
}

impl std::fmt::Display for ActionRejection {
    /// Formats the reason code for diagnostics without inventing presentation text.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.code)
    }
}

impl std::error::Error for ActionRejection {}

/// Defines one two-player game's deterministic mechanics and role information.
///
/// `State`, `Action`, `Observation`, and `Completion` are structured domain data,
/// not rendered human interfaces. The same observations and actions are consumed
/// by browser clients and agents. Given the same configuration, seed, state, and
/// action sequence, an implementation should produce the same results; stochastic
/// games should initialize their random state from `seed` and store it in `State`.
pub trait Game: Send + Sync + 'static {
    /// Game-owned per-experiment configuration edited through the dashboard.
    type Config: Serialize + DeserializeOwned + Send + Sync + 'static;
    /// Authoritative game state owned by the runtime and never sent to participants.
    type State: Serialize + Send + Sync + 'static;
    /// Structured game operation proposed by either player role.
    type Action: Serialize + DeserializeOwned + Clone + Send + Sync + 'static;
    /// Complete game information currently available to one player role.
    type Observation: Serialize + Send + Sync + 'static;
    /// Structured terminal result sent to both players and stored for analysis.
    ///
    /// This shared value may contain public facts such as the winner and scores.
    /// Put role-private terminal facts only in that role's final [`Game::Observation`].
    type Completion: Serialize + Clone + Send + Sync + 'static;

    /// Creates a fresh authoritative state from game configuration and a recorded seed.
    fn initial_state(
        &self,
        context: GameInitializationContext<'_, Self::Config>,
    ) -> Result<Self::State>;

    /// Applies one typed action or returns an expected machine-readable rule rejection.
    ///
    /// Implementations must validate the actor and all state-dependent legality in
    /// this operation. The method must not perform I/O. Runtime failures such as
    /// persistence errors are handled separately by the server.
    fn apply_action(
        &self,
        state: &Self::State,
        action: &Self::Action,
        actor: PlayerRole,
    ) -> std::result::Result<Self::State, ActionRejection>;

    /// Returns the complete role-specific information state for one player role.
    fn observation(&self, state: &Self::State, role: PlayerRole) -> Self::Observation;

    /// Lists a role's discrete action affordances when the game provides a catalogue.
    ///
    /// `None` means the action space is not enumerated. `Some(vec![])` means the
    /// game provides an enumeration and no action is currently available. Every
    /// submitted action is still checked by `apply_action`.
    fn available_actions(
        &self,
        _state: &Self::State,
        _role: PlayerRole,
    ) -> Option<Vec<Self::Action>> {
        None
    }

    /// Adds optional role-neutral domain metadata to the durable transition log.
    ///
    /// This value is for analysis and dashboard inspection. It must not contain
    /// viewer-specific prose or assumptions about how a human interface renders
    /// the transition. Parlando always records the actor and action itself.
    fn transition_metadata(
        &self,
        _before: &Self::State,
        _after: &Self::State,
        _action: &Self::Action,
        _actor: PlayerRole,
    ) -> Option<Value> {
        None
    }

    /// Returns the shared structured terminal result, or `None` while play continues.
    ///
    /// The result is delivered unchanged to both players and stored for analysis.
    /// It may contain public game-specific facts such as winner, win/loss outcome,
    /// and scores. Put role-private terminal facts in the role's final observation.
    fn completion(&self, state: &Self::State) -> Option<Self::Completion>;
}

/// Reusable constructor for session-local values of one compiled game.
///
/// The server owns one factory for the life of the process. Each call to
/// [`GameFactory::create`] receives the context for a newly admitted session and
/// must return a fresh [`Game`] value owned exclusively by that session.
pub trait GameFactory: Send + Sync + 'static {
    /// Session-owned game type created by this reusable factory.
    type Game: Game;

    /// Creates a game instance which belongs to exactly one execution session.
    fn create(&self, context: GameSessionContext) -> Result<Self::Game>;

    /// Validates semantic constraints on already-deserialized game configuration.
    fn validate_config(&self, _config: &<Self::Game as Game>::Config) -> Result<()> {
        Ok(())
    }
}

/// Deserializes and semantically validates one stored game configuration value.
pub(crate) fn parse_game_config<G: Game>(
    factory: &dyn GameFactory<Game = G>,
    value: &Value,
) -> Result<G::Config> {
    let config = serde_json::from_value(value.clone())?;
    factory.validate_config(&config)?;
    Ok(config)
}

#[cfg(test)]
mod config_tests {
    use super::*;
    use serde_json::json;

    /// Exercises recursive typed normalization and strict unknown-field rejection.
    #[test]
    fn semantic_agent_settings_are_normalized_strictly() {
        let definition = AgentDefinition {
            id: "test".into(),
            name: "Test".into(),
            description: String::new(),
            config_fields: vec![AgentConfigField {
                key: "nested".into(),
                label: "Nested".into(),
                help: String::new(),
                required: true,
                default_value: Value::Null,
                value: AgentConfigValue::Object {
                    fields: vec![AgentConfigField {
                        key: "count".into(),
                        label: "Count".into(),
                        help: String::new(),
                        required: false,
                        default_value: json!(3),
                        value: AgentConfigValue::Integer {
                            minimum: Some(1),
                            maximum: Some(4),
                        },
                    }],
                },
            }],
        };
        definition.validate().unwrap();
        assert_eq!(
            definition
                .normalize_settings(&json!({"nested": {}}))
                .unwrap(),
            json!({"nested": {"count": 3}})
        );
        assert!(definition
            .normalize_settings(&json!({"nested": {"unknown": true}}))
            .is_err());
        assert!(definition
            .normalize_settings(&json!({"nested": {"count": 5}}))
            .is_err());
    }

    /// Debug formatting never exposes secret values.
    #[test]
    fn secret_values_debug_is_redacted() {
        let values = SecretValues::new(BTreeMap::from([(
            "config.token".into(),
            "sentinel-secret".into(),
        )]));
        let debug = format!("{values:?}");
        assert!(debug.contains("REDACTED"));
        assert!(!debug.contains("sentinel-secret"));
    }

    /// Secret references resolve only into their declared delivery subsets.
    #[test]
    fn secret_references_resolve_by_purpose() {
        let field = |key: &str, purpose| AgentConfigField {
            key: key.into(),
            label: key.into(),
            help: String::new(),
            required: true,
            default_value: Value::Null,
            value: AgentConfigValue::SecretReference { purpose },
        };
        let definition = AgentDefinition {
            id: "secrets".into(),
            name: "Secrets".into(),
            description: String::new(),
            config_fields: vec![
                field("transport", SecretPurpose::Factory),
                field("instance", SecretPurpose::AgentInstance),
            ],
        };
        let settings = definition
            .normalize_settings(&json!({"transport": "game.auth", "instance": "game.api"}))
            .unwrap();
        let values = std::collections::HashMap::from([
            ("auth".into(), "transport-value".into()),
            ("api".into(), "instance-value".into()),
            ("unrelated".into(), "hidden".into()),
        ]);
        let (factory, instance) = definition.resolve_secrets(&settings, &values).unwrap();
        assert_eq!(factory.get("config.transport"), Some("transport-value"));
        assert_eq!(instance.get("config.instance"), Some("instance-value"));
        assert_eq!(factory.get("config.instance"), None);
        assert_eq!(instance.get("unrelated"), None);
    }
}
