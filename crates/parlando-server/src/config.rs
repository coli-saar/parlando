use std::{collections::HashMap, env, fs, path::{Path, PathBuf}};

use anyhow::{anyhow, bail, Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct StudyConfig {
    pub name: String,
    pub enabled_sources: Vec<String>,
    pub waiting_room_timeout_seconds: i64,
    pub reconnect_grace_seconds: i64,
}

impl Default for StudyConfig {
    fn default() -> Self {
        Self {
            name: "experiment".to_string(),
            enabled_sources: vec!["direct".to_string()],
            waiting_room_timeout_seconds: 300,
            reconnect_grace_seconds: 90,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct ConsentItemConfig {
    pub id: String,
    pub title: String,
    pub body_html: String,
    pub required: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct DirectConfig {
    pub enabled: bool,
    pub allow_room_codes: bool,
    pub allow_matchmaking: bool,
    pub require_consent: bool,
    pub consents: Vec<ConsentItemConfig>,
}

impl Default for DirectConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            allow_room_codes: true,
            allow_matchmaking: true,
            require_consent: false,
            consents: vec![],
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct LegacyGameConfig {
    pub adapter: Option<String>,
    pub module_paths: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct ServerConfig {
    pub public_base_url: String,
    pub allowed_origins: Vec<String>,
    pub client_dist_path: Option<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            public_base_url: "http://localhost:8000".to_string(),
            allowed_origins: vec![],
            client_dist_path: None,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct DatabaseConfig {
    pub url: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct LiveKitConfig {
    pub enabled: bool,
    pub url: String,
    pub api_key: String,
    pub api_secret: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct SpeechmaticsConfig {
    pub enabled: bool,
    pub api_key: String,
    pub realtime_url: String,
    pub temporary_key_ttl_seconds: i64,
    pub management_url: String,
    pub max_delay: f64,
    pub enable_partials: bool,
    pub end_of_utterance_silence_trigger: f64,
}

impl Default for SpeechmaticsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            api_key: String::new(),
            realtime_url: "wss://eu.rt.speechmatics.com/v2".to_string(),
            temporary_key_ttl_seconds: 60,
            management_url: "https://mp.speechmatics.com/v1/api_keys".to_string(),
            max_delay: 2.0,
            enable_partials: true,
            end_of_utterance_silence_trigger: 1.2,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct TranscriptionConfig {
    pub enabled: bool,
    pub provider: String,
    pub model: String,
    pub language: String,
    pub worker_autostart: bool,
    pub store_audio: bool,
}

impl Default for TranscriptionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: "livekit".to_string(),
            model: "deepgram/nova-3".to_string(),
            language: "en-US".to_string(),
            worker_autostart: true,
            store_audio: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct TtsConfig {
    pub enabled: bool,
    pub provider: String,
    pub model: String,
    pub voice_id: String,
    pub voice_name: String,
    pub api_key: String,
    pub output_format: String,
    pub worker_autostart: bool,
}

impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: "elevenlabs".to_string(),
            model: "eleven_flash_v2_5".to_string(),
            voice_id: String::new(),
            voice_name: String::new(),
            api_key: String::new(),
            output_format: "pcm_24000".to_string(),
            worker_autostart: true,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct ConversationConfig {
    pub enabled: bool,
    pub max_history_messages: usize,
}

impl Default for ConversationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_history_messages: 50,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct HumanVsAgentConfig {
    pub factory: Option<String>,
    pub max_concurrent_games: usize,
    pub act_timeout_seconds: f64,
    pub invalid_action_limit: usize,
    pub seed: Option<u64>,
    pub config: Value,
}

impl Default for HumanVsAgentConfig {
    fn default() -> Self {
        Self {
            factory: None,
            max_concurrent_games: 20,
            act_timeout_seconds: 10.0,
            invalid_action_limit: 3,
            seed: None,
            config: Value::Object(Default::default()),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentsMode {
    HumanVsHuman,
    HumanVsAgent,
}

impl Default for AgentsMode {
    fn default() -> Self {
        Self::HumanVsHuman
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct AgentsConfig {
    pub mode: AgentsMode,
    pub human_vs_agent: Option<HumanVsAgentConfig>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct ExperimentConfig {
    pub study: StudyConfig,
    pub game: LegacyGameConfig,
    pub direct: DirectConfig,
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub livekit: LiveKitConfig,
    pub speechmatics: SpeechmaticsConfig,
    pub transcription: TranscriptionConfig,
    pub tts: TtsConfig,
    pub conversation: ConversationConfig,
    pub agents: AgentsConfig,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

impl Default for ExperimentConfig {
    fn default() -> Self {
        Self {
            study: StudyConfig::default(),
            game: LegacyGameConfig { adapter: None, module_paths: vec!["src".to_string()] },
            direct: DirectConfig::default(),
            server: ServerConfig::default(),
            database: DatabaseConfig::default(),
            livekit: LiveKitConfig::default(),
            speechmatics: SpeechmaticsConfig::default(),
            transcription: TranscriptionConfig::default(),
            tts: TtsConfig::default(),
            conversation: ConversationConfig::default(),
            agents: AgentsConfig::default(),
            extra: HashMap::new(),
        }
    }
}

impl ExperimentConfig {
    pub fn from_yaml(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().canonicalize().with_context(|| {
            format!("failed to resolve config path {}", path.as_ref().display())
        })?;
        let mut data = load_yaml_with_includes(&path, &mut vec![])?;
        resolve_relative_paths(&mut data, &config_base_path(&path));
        apply_env_overrides(&mut data);
        let config: Self = serde_json::from_value(data)?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        if self.tts.enabled {
            if self.tts.voice_id.is_empty() {
                bail!("tts.voice_id is required when TTS is enabled");
            }
            if self.tts.api_key.is_empty() {
                bail!("tts.api_key is required when TTS is enabled");
            }
        }
        if self.livekit.enabled
            && (self.livekit.url.is_empty()
                || self.livekit.api_key.is_empty()
                || self.livekit.api_secret.is_empty())
        {
            bail!("livekit.url, livekit.api_key, and livekit.api_secret are required when LiveKit is enabled");
        }
        if self.transcription.enabled
            && self.transcription.provider == "speechmatics"
            && (!self.speechmatics.enabled || self.speechmatics.api_key.is_empty())
        {
            bail!("speechmatics.enabled and speechmatics.api_key are required for Speechmatics transcription");
        }
        Ok(())
    }
}

fn load_yaml_with_includes(path: &Path, seen: &mut Vec<PathBuf>) -> Result<Value> {
    if seen.iter().any(|candidate| candidate == path) {
        bail!("config include cycle detected at {}", path.display());
    }
    seen.push(path.to_path_buf());
    let text = expand_env(&fs::read_to_string(path)?);
    let yaml: serde_yaml::Value = serde_yaml::from_str(&text)?;
    let mut data = serde_json::to_value(yaml)?;
    let parent = take_field(&mut data, "extends");
    let includes = take_field(&mut data, "includes").or_else(|| take_field(&mut data, "include"));

    if let Some(parent) = parent {
        let parent_path = resolve_include_path(path, value_to_path(&parent)?)?;
        let parent_data = load_yaml_with_includes(&parent_path, seen)?;
        data = deep_merge(parent_data, data);
    }

    for include in normalize_includes(includes)? {
        let include_path = resolve_include_path(path, include.path)?;
        if !include_path.exists() {
            if include.optional {
                continue;
            }
            return Err(anyhow!("config include not found: {}", include_path.display()));
        }
        let include_data = load_yaml_with_includes(&include_path, seen)?;
        data = deep_merge(data, include_data);
    }

    seen.pop();
    Ok(data)
}

#[derive(Debug)]
struct IncludeEntry {
    path: PathBuf,
    optional: bool,
}

fn normalize_includes(value: Option<Value>) -> Result<Vec<IncludeEntry>> {
    let Some(value) = value else { return Ok(vec![]) };
    let values = match value {
        Value::Array(values) => values,
        other => vec![other],
    };
    values
        .into_iter()
        .map(|value| match value {
            Value::String(path) => Ok(IncludeEntry { path: PathBuf::from(path), optional: false }),
            Value::Object(map) => {
                let path = map.get("path").and_then(Value::as_str).context("include entries need path")?;
                let optional = map.get("optional").and_then(Value::as_bool).unwrap_or(false);
                Ok(IncludeEntry { path: PathBuf::from(path), optional })
            }
            _ => bail!("includes must be strings or objects with path/optional"),
        })
        .collect()
}

fn take_field(value: &mut Value, field: &str) -> Option<Value> {
    value.as_object_mut()?.remove(field)
}

fn value_to_path(value: &Value) -> Result<PathBuf> {
    value.as_str().map(PathBuf::from).context("include path must be a string")
}

fn resolve_include_path(config_path: &Path, include: PathBuf) -> Result<PathBuf> {
    Ok(if include.is_absolute() {
        include
    } else {
        config_path.parent().unwrap_or_else(|| Path::new(".")).join(include)
    }
    .canonicalize()
    .unwrap_or_else(|_| config_path.parent().unwrap_or_else(|| Path::new(".")).join("")))
}

fn deep_merge(base: Value, override_value: Value) -> Value {
    match (base, override_value) {
        (Value::Object(mut base), Value::Object(override_map)) => {
            for (key, value) in override_map {
                let merged = match base.remove(&key) {
                    Some(base_value) => deep_merge(base_value, value),
                    None => value,
                };
                base.insert(key, merged);
            }
            Value::Object(base)
        }
        (_, override_value) => override_value,
    }
}

fn config_base_path(path: &Path) -> PathBuf {
    if path.parent().and_then(Path::file_name).and_then(|s| s.to_str()) == Some("config") {
        path.parent().and_then(Path::parent).unwrap_or_else(|| Path::new(".")).to_path_buf()
    } else {
        path.parent().unwrap_or_else(|| Path::new(".")).to_path_buf()
    }
}

fn resolve_relative_paths(data: &mut Value, base: &Path) {
    if let Some(server) = data.get_mut("server").and_then(Value::as_object_mut) {
        if let Some(path) = server.get("client_dist_path").and_then(Value::as_str) {
            let path = PathBuf::from(path);
            if !path.is_absolute() {
                server.insert("client_dist_path".to_string(), Value::String(base.join(path).display().to_string()));
            }
        }
    }
    if let Some(database) = data.get_mut("database").and_then(Value::as_object_mut) {
        if let Some(url) = database.get("url").and_then(Value::as_str) {
            if url.starts_with("sqlite:///") && !url.starts_with("sqlite:////") && url != "sqlite:///:memory:" {
                let raw = url.trim_start_matches("sqlite:///");
                database.insert("url".to_string(), Value::String(format!("sqlite:///{}", base.join(raw).display())));
            }
        }
    }
}

fn apply_env_overrides(data: &mut Value) {
    if let Ok(mode) = env::var("EXPERIMENT_AGENTS_MODE") {
        if !mode.is_empty() {
            let root = data.as_object_mut().expect("config root must be object");
            let agents = root.entry("agents").or_insert_with(|| Value::Object(Default::default()));
            agents.as_object_mut().expect("agents must be object").insert("mode".to_string(), Value::String(mode));
        }
    }
}

fn expand_env(text: &str) -> String {
    let pattern = Regex::new(r"\$\{([A-Z0-9_]+)\}").expect("valid env regex");
    pattern
        .replace_all(text, |captures: &regex::Captures| env::var(&captures[1]).unwrap_or_default())
        .to_string()
}
