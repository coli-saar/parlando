use std::{
    collections::HashMap,
    env, fs,
    path::{Path, PathBuf},
};

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
    pub require_consent: bool,
    pub participant_information_version: String,
    pub participant_information_url: String,
    pub consents: Vec<ConsentItemConfig>,
}

impl Default for DirectConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            allow_room_codes: true,
            require_consent: false,
            participant_information_version: String::new(),
            participant_information_url: String::new(),
            consents: vec![],
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct PrivacyConfig {
    /// Version of the stable privacy behavior implemented by this server release.
    pub contract_version: String,
    /// Persists complete game-state snapshots alongside accepted transitions.
    pub store_full_game_state: bool,
    /// Persists participant messages whose existing origin is `typed`.
    pub store_typed_messages: bool,
    /// Persists final transcript and corresponding conversation-message events.
    pub store_final_transcripts: bool,
    /// Persists minimized voice diagnostic events.
    pub store_voice_diagnostics: bool,
}

impl Default for PrivacyConfig {
    /// Uses research-friendly content defaults while disabling optional diagnostics.
    fn default() -> Self {
        Self {
            contract_version: "1".to_string(),
            store_full_game_state: true,
            store_typed_messages: true,
            store_final_transcripts: true,
            store_voice_diagnostics: false,
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
pub struct ExperimentIdentityConfig {
    pub id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct VoiceConfig {
    /// Enables Parlando's authenticated browser-to-server audio WebSocket.
    pub enabled: bool,
    /// Canonical PCM sample rate; protocol version 1 requires 24 kHz.
    pub sample_rate_hz: u32,
    /// Canonical binary frame duration; protocol version 1 requires 20 ms.
    pub frame_duration_ms: u16,
    /// Browser playback buffer target before audio starts or resumes.
    pub jitter_buffer_ms: u16,
}

impl Default for VoiceConfig {
    /// Uses the single wire format implemented by protocol version 1.
    fn default() -> Self {
        Self {
            enabled: false,
            sample_rate_hz: 24_000,
            frame_duration_ms: 20,
            jitter_buffer_ms: 100,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct SpeechmaticsConfig {
    pub api_key: String,
    pub realtime_url: String,
    pub max_delay: f64,
    pub enable_partials: bool,
    pub end_of_utterance_silence_trigger: f64,
}

impl Default for SpeechmaticsConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            realtime_url: "wss://eu.rt.speechmatics.com/v2".to_string(),
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
    pub store_audio: bool,
}

impl Default for TranscriptionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: "speechmatics".to_string(),
            model: "enhanced".to_string(),
            language: "en-US".to_string(),
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

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct AgentOptionConfig {
    pub selector: String,
    pub label: String,
    pub description: Option<String>,
    pub requires_config: bool,
    pub default_config: Value,
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
    pub available: Vec<AgentOptionConfig>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct ExperimentConfig {
    pub experiment: ExperimentIdentityConfig,
    pub study: StudyConfig,
    pub game: LegacyGameConfig,
    pub direct: DirectConfig,
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub voice: VoiceConfig,
    pub speechmatics: SpeechmaticsConfig,
    pub transcription: TranscriptionConfig,
    pub tts: TtsConfig,
    pub conversation: ConversationConfig,
    pub agents: AgentsConfig,
    pub privacy: PrivacyConfig,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

impl Default for ExperimentConfig {
    fn default() -> Self {
        Self {
            study: StudyConfig::default(),
            experiment: ExperimentIdentityConfig::default(),
            game: LegacyGameConfig {
                adapter: None,
                module_paths: vec!["src".to_string()],
            },
            direct: DirectConfig::default(),
            server: ServerConfig::default(),
            database: DatabaseConfig::default(),
            voice: VoiceConfig::default(),
            speechmatics: SpeechmaticsConfig::default(),
            transcription: TranscriptionConfig::default(),
            tts: TtsConfig::default(),
            conversation: ConversationConfig::default(),
            agents: AgentsConfig::default(),
            privacy: PrivacyConfig::default(),
            extra: HashMap::new(),
        }
    }
}

impl ExperimentConfig {
    /// Loads an experiment configuration from YAML using the Python-compatible include semantics.
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

    /// Validates cross-field requirements that cannot be expressed by serde defaults alone.
    pub fn validate(&self) -> Result<()> {
        if self.privacy.contract_version.trim().is_empty() {
            bail!("privacy.contract_version must not be empty");
        }
        if self.direct.require_consent
            && (self
                .direct
                .participant_information_version
                .trim()
                .is_empty()
                || self.direct.participant_information_url.trim().is_empty())
        {
            bail!("direct participant information version and URL are required when consent is enabled");
        }
        if self.tts.enabled {
            if self.tts.voice_id.is_empty() {
                bail!("tts.voice_id is required when TTS is enabled");
            }
            if self.tts.api_key.is_empty() {
                bail!("tts.api_key is required when TTS is enabled");
            }
        }
        if self.voice.enabled
            && (self.voice.sample_rate_hz != 24_000 || self.voice.frame_duration_ms != 20)
        {
            bail!(
                "voice protocol version 1 requires sample_rate_hz 24000 and frame_duration_ms 20"
            );
        }
        if self.transcription.enabled
            && self.transcription.provider == "speechmatics"
            && self.speechmatics.api_key.is_empty()
        {
            bail!("speechmatics.api_key is required for Speechmatics transcription");
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
            return Err(anyhow!(
                "config include not found: {}",
                include_path.display()
            ));
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
    let Some(value) = value else {
        return Ok(vec![]);
    };
    let values = match value {
        Value::Array(values) => values,
        other => vec![other],
    };
    values
        .into_iter()
        .map(|value| match value {
            Value::String(path) => Ok(IncludeEntry {
                path: PathBuf::from(path),
                optional: false,
            }),
            Value::Object(map) => {
                let path = map
                    .get("path")
                    .and_then(Value::as_str)
                    .context("include entries need path")?;
                let optional = map
                    .get("optional")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                Ok(IncludeEntry {
                    path: PathBuf::from(path),
                    optional,
                })
            }
            _ => bail!("includes must be strings or objects with path/optional"),
        })
        .collect()
}

fn take_field(value: &mut Value, field: &str) -> Option<Value> {
    value.as_object_mut()?.remove(field)
}

fn value_to_path(value: &Value) -> Result<PathBuf> {
    value
        .as_str()
        .map(PathBuf::from)
        .context("include path must be a string")
}

fn resolve_include_path(config_path: &Path, include: PathBuf) -> Result<PathBuf> {
    let resolved = if include.is_absolute() {
        include
    } else {
        config_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(include)
    };
    Ok(resolved.canonicalize().unwrap_or(resolved))
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
    if path
        .parent()
        .and_then(Path::file_name)
        .and_then(|s| s.to_str())
        == Some("config")
    {
        path.parent()
            .and_then(Path::parent)
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf()
    } else {
        path.parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf()
    }
}

fn resolve_relative_paths(data: &mut Value, base: &Path) {
    if let Some(server) = data.get_mut("server").and_then(Value::as_object_mut) {
        if let Some(path) = server.get("client_dist_path").and_then(Value::as_str) {
            let path = PathBuf::from(path);
            if !path.is_absolute() {
                server.insert(
                    "client_dist_path".to_string(),
                    Value::String(base.join(path).display().to_string()),
                );
            }
        }
    }
    if let Some(database) = data.get_mut("database").and_then(Value::as_object_mut) {
        if let Some(url) = database.get("url").and_then(Value::as_str) {
            if url.starts_with("sqlite:///")
                && !url.starts_with("sqlite:////")
                && url != "sqlite:///:memory:"
            {
                let raw = url.trim_start_matches("sqlite:///");
                database.insert(
                    "url".to_string(),
                    Value::String(format!("sqlite:///{}", base.join(raw).display())),
                );
            }
        }
    }
}

fn apply_env_overrides(data: &mut Value) {
    if let Ok(mode) = env::var("EXPERIMENT_AGENTS_MODE") {
        if !mode.is_empty() {
            let root = data.as_object_mut().expect("config root must be object");
            let agents = root
                .entry("agents")
                .or_insert_with(|| Value::Object(Default::default()));
            agents
                .as_object_mut()
                .expect("agents must be object")
                .insert("mode".to_string(), Value::String(mode));
        }
    }
}

fn expand_env(text: &str) -> String {
    let pattern = Regex::new(r"\$\{([A-Z0-9_]+)\}").expect("valid env regex");
    pattern
        .replace_all(text, |captures: &regex::Captures| {
            env::var(&captures[1]).unwrap_or_default()
        })
        .to_string()
}

#[cfg(test)]
mod tests {
    use std::{fs, sync::Mutex};

    use tempfile::tempdir;

    use super::*;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn yaml_extends_includes_env_and_relative_paths_match_expected_behavior() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        env::set_var("PARLANDO_TEST_STUDY", "Env Study");
        env::set_var("EXPERIMENT_AGENTS_MODE", "human_vs_agent");

        let temp = tempdir().expect("tempdir");
        let project = temp.path();
        let config_dir = project.join("config");
        fs::create_dir_all(&config_dir).expect("config dir");
        fs::write(
            config_dir.join("base.yaml"),
            r#"
study:
  name: base
direct:
  require_consent: true
  participant_information_version: test-v1
  participant_information_url: https://example.test/privacy
database:
  url: sqlite:///data/parlando.sqlite
server:
  client_dist_path: dist
"#,
        )
        .expect("base config");
        fs::write(
            config_dir.join("include.yaml"),
            r#"
conversation:
  max_history_messages: 12
"#,
        )
        .expect("include config");
        fs::write(
            config_dir.join("experiment.yaml"),
            r#"
extends: base.yaml
includes:
  - include.yaml
  - path: missing-private.yaml
    optional: true
study:
  name: ${PARLANDO_TEST_STUDY}
agents:
  human_vs_agent:
    factory: space_game.back_and_forth
"#,
        )
        .expect("experiment config");

        let config =
            ExperimentConfig::from_yaml(config_dir.join("experiment.yaml")).expect("config loads");

        assert_eq!(config.study.name, "Env Study");
        assert!(config.direct.require_consent);
        assert_eq!(config.conversation.max_history_messages, 12);
        assert_eq!(config.agents.mode, AgentsMode::HumanVsAgent);
        let canonical_project = config_dir
            .canonicalize()
            .expect("canonical config dir")
            .parent()
            .expect("project dir")
            .to_path_buf();
        assert_eq!(
            config.server.client_dist_path.as_deref(),
            Some(canonical_project.join("dist").to_str().unwrap())
        );
        assert_eq!(
            config.database.url,
            format!(
                "sqlite:///{}",
                canonical_project.join("data/parlando.sqlite").display()
            )
        );

        env::remove_var("PARLANDO_TEST_STUDY");
        env::remove_var("EXPERIMENT_AGENTS_MODE");
    }

    #[test]
    fn yaml_required_include_must_exist() {
        let temp = tempdir().expect("tempdir");
        let path = temp.path().join("experiment.yaml");
        fs::write(&path, "includes: missing.yaml\n").expect("config");

        let error = ExperimentConfig::from_yaml(path).expect_err("missing required include fails");

        assert!(
            error.to_string().contains("Config include not found")
                || error.to_string().contains("config include not found")
        );
    }

    #[test]
    fn validation_requires_enabled_tts_secrets() {
        let mut config = ExperimentConfig::default();
        config.tts.enabled = true;
        config.tts.voice_id = "voice".to_string();

        let error = config.validate().expect_err("missing api key fails");

        assert!(error.to_string().contains("tts.api_key"));
    }
}
