use std::collections::{HashMap, HashSet};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Runtime-owned lifecycle limits for one game session.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SessionConfig {
    /// Maximum time a one-player session may wait for its second role.
    pub waiting_session_timeout_seconds: i64,
    /// Maximum time a disconnected player may reclaim an active role.
    pub reconnect_grace_seconds: i64,
    /// Maximum inactivity allowed after a session starts before it is expired.
    pub session_idle_timeout_seconds: i64,
    /// Maximum wall-clock lifetime of a session, including waiting time.
    pub session_max_lifetime_seconds: i64,
}

impl Default for SessionConfig {
    /// Uses conservative lifecycle limits for a modest research deployment.
    fn default() -> Self {
        Self {
            waiting_session_timeout_seconds: 10 * 60,
            reconnect_grace_seconds: 5 * 60,
            session_idle_timeout_seconds: 30 * 60,
            session_max_lifetime_seconds: 4 * 60 * 60,
        }
    }
}

/// Installation capacity reserved coherently when a research session is admitted.
///
/// These values are ordinary experiment configuration. They are edited through the
/// administrator dashboard and persisted with the experiment revision; deployments
/// must not introduce environment-variable overrides for them.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct CapacityConfig {
    /// Sessions that may be running or retained during their reconnect grace period.
    pub max_active_sessions: usize,
    /// Human-human sessions that may wait for their second participant.
    pub max_waiting_sessions: usize,
    /// Participant credentials that may exist before they have joined a session.
    pub max_unattached_participants: usize,
    /// Browser ASR streams reserved across waiting and active sessions.
    pub max_transcription_streams: usize,
    /// Free disk space kept available before new sessions are paused.
    pub storage_reserve_megabytes: u64,
}

impl Default for CapacityConfig {
    /// Uses conservative defaults suitable for one modest self-hosted research server.
    fn default() -> Self {
        Self {
            max_active_sessions: 30,
            max_waiting_sessions: 60,
            max_unattached_participants: 120,
            max_transcription_streams: 32,
            storage_reserve_megabytes: 256,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ConsentItemConfig {
    pub id: String,
    pub title: String,
    /// Plain-text consent copy displayed without HTML interpretation.
    pub body: String,
    pub required: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct DirectConfig {
    pub enabled: bool,
    pub participant_information_version: String,
    pub participant_information_url: String,
    pub consents: Vec<ConsentItemConfig>,
}

impl Default for DirectConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            participant_information_version: String::new(),
            participant_information_url: String::new(),
            consents: vec![],
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct PrivacyConfig {
    /// Version of the stable privacy behavior implemented by this server release.
    pub contract_version: String,
}

impl Default for PrivacyConfig {
    /// Selects the current fixed privacy contract.
    fn default() -> Self {
        Self {
            contract_version: "2".to_string(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
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
#[serde(default, deny_unknown_fields)]
pub struct DatabaseConfig {
    pub url: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExperimentIdentityConfig {
    pub id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
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
#[serde(default, deny_unknown_fields)]
pub struct SpeechmaticsConfig {
    #[serde(skip)]
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
#[serde(default, deny_unknown_fields)]
pub struct TranscriptionConfig {
    pub enabled: bool,
    pub provider: String,
    pub model: String,
    pub language: String,
}

impl Default for TranscriptionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: "speechmatics".to_string(),
            model: "enhanced".to_string(),
            language: "en-US".to_string(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct TtsConfig {
    pub enabled: bool,
    pub provider: String,
    pub model: String,
    pub voice_id: String,
    pub voice_name: String,
    /// Revisioned WebSocket service origin used for synthesized speech.
    pub base_url: String,
    #[serde(skip)]
    pub api_key: String,
    pub output_format: String,
}

impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: "elevenlabs".to_string(),
            model: "eleven_flash_v2_5".to_string(),
            voice_id: String::new(),
            voice_name: String::new(),
            base_url: "wss://api.elevenlabs.io".to_string(),
            api_key: String::new(),
            output_format: "pcm_24000".to_string(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct HumanVsAgentConfig {
    pub factory: Option<String>,
    pub act_timeout_seconds: f64,
    pub invalid_action_limit: usize,
    pub seed: Option<u64>,
    pub config: Value,
}

impl Default for HumanVsAgentConfig {
    fn default() -> Self {
        Self {
            factory: None,
            act_timeout_seconds: 10.0,
            invalid_action_limit: 3,
            seed: None,
            config: Value::Object(Default::default()),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentsMode {
    #[default]
    HumanVsHuman,
    HumanVsAgent,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentsConfig {
    pub mode: AgentsMode,
    pub human_vs_agent: Option<HumanVsAgentConfig>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExperimentConfig {
    #[serde(skip)]
    pub experiment: ExperimentIdentityConfig,
    pub session: SessionConfig,
    pub direct: DirectConfig,
    #[serde(skip)]
    pub server: ServerConfig,
    #[serde(skip)]
    pub database: DatabaseConfig,
    pub voice: VoiceConfig,
    pub speechmatics: SpeechmaticsConfig,
    pub transcription: TranscriptionConfig,
    pub tts: TtsConfig,
    pub agents: AgentsConfig,
    pub capacity: CapacityConfig,
    pub privacy: PrivacyConfig,
    /// Game-owned, per-experiment options edited as YAML in the dashboard.
    pub game: Value,
    /// Write-only game credentials loaded from the experiment secret store.
    #[serde(skip)]
    pub game_secrets: HashMap<String, String>,
}

impl Default for ExperimentConfig {
    /// Creates a complete experiment configuration with an empty game-owned mapping.
    fn default() -> Self {
        Self {
            experiment: ExperimentIdentityConfig::default(),
            session: SessionConfig::default(),
            direct: DirectConfig::default(),
            server: ServerConfig::default(),
            database: DatabaseConfig::default(),
            voice: VoiceConfig::default(),
            speechmatics: SpeechmaticsConfig::default(),
            transcription: TranscriptionConfig::default(),
            tts: TtsConfig::default(),
            agents: AgentsConfig::default(),
            capacity: CapacityConfig::default(),
            privacy: PrivacyConfig::default(),
            game: Value::Object(Default::default()),
            game_secrets: HashMap::new(),
        }
    }
}

impl ExperimentConfig {
    /// Validates cross-field requirements that cannot be expressed by serde defaults alone.
    pub fn validate(&self) -> Result<()> {
        if self.experiment.id.as_ref().is_some_and(|id| {
            id.is_empty()
                || id.chars().count() > 128
                || !id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        }) {
            bail!(
                "experiment.id must contain 1 to 128 letters, digits, dots, dashes, or underscores"
            );
        }
        if self.session.waiting_session_timeout_seconds <= 0 {
            bail!("session.waiting_session_timeout_seconds must be positive");
        }
        if self.session.reconnect_grace_seconds < 0 {
            bail!("session.reconnect_grace_seconds must not be negative");
        }
        if self.session.session_idle_timeout_seconds <= 0 {
            bail!("session.session_idle_timeout_seconds must be positive");
        }
        if self.session.session_max_lifetime_seconds < self.session.session_idle_timeout_seconds {
            bail!(
                "session.session_max_lifetime_seconds must be at least session.session_idle_timeout_seconds"
            );
        }
        if self.capacity.max_active_sessions == 0
            || self.capacity.max_waiting_sessions == 0
            || self.capacity.max_unattached_participants == 0
            || self.capacity.max_transcription_streams == 0
            || self.capacity.storage_reserve_megabytes == 0
        {
            bail!("capacity limits must be positive");
        }
        validate_http_url("server.public_base_url", &self.server.public_base_url)?;
        let mut allowed_origins = HashSet::new();
        for origin in &self.server.allowed_origins {
            validate_http_url("server.allowed_origins", origin)?;
            let uri: http::Uri = origin.parse()?;
            if uri.path() != "/" || uri.query().is_some() {
                bail!("server.allowed_origins entries must not contain paths or queries");
            }
            if !allowed_origins.insert(origin.to_ascii_lowercase()) {
                bail!("server.allowed_origins entries must be unique");
            }
        }
        if self.database.url.trim().is_empty() {
            bail!("database.url must not be empty");
        }
        if self.privacy.contract_version.trim().is_empty() {
            bail!("privacy.contract_version must not be empty");
        }
        if self.voice.jitter_buffer_ms < self.voice.frame_duration_ms
            || self.voice.jitter_buffer_ms > 5_000
        {
            bail!("voice.jitter_buffer_ms must be between one frame and 5000 ms");
        }
        if self.tts.enabled {
            if self.tts.provider != "elevenlabs" {
                bail!("tts.provider must be elevenlabs");
            }
            if self.tts.model.trim().is_empty() {
                bail!("tts.model is required when TTS is enabled");
            }
            if self.tts.output_format != "pcm_24000" {
                bail!("tts.output_format must be pcm_24000");
            }
            validate_websocket_url("tts.base_url", &self.tts.base_url)?;
        }
        if self.voice.enabled
            && (self.voice.sample_rate_hz != 24_000 || self.voice.frame_duration_ms != 20)
        {
            bail!(
                "voice protocol version 1 requires sample_rate_hz 24000 and frame_duration_ms 20"
            );
        }
        if self.transcription.enabled {
            if self.transcription.provider != "speechmatics" {
                bail!("transcription.provider must be speechmatics");
            }
            if !matches!(self.transcription.model.as_str(), "standard" | "enhanced") {
                bail!("transcription.model must be standard or enhanced");
            }
            if self.transcription.model.trim().is_empty()
                || self.transcription.language.trim().is_empty()
            {
                bail!("transcription.model and transcription.language must not be empty");
            }
            validate_websocket_url("speechmatics.realtime_url", &self.speechmatics.realtime_url)?;
            if !self.speechmatics.max_delay.is_finite() || self.speechmatics.max_delay <= 0.0 {
                bail!("speechmatics.max_delay must be finite and positive");
            }
            if !self
                .speechmatics
                .end_of_utterance_silence_trigger
                .is_finite()
                || self.speechmatics.end_of_utterance_silence_trigger <= 0.0
            {
                bail!("speechmatics.end_of_utterance_silence_trigger must be finite and positive");
            }
        }
        if let Some(agent) = &self.agents.human_vs_agent {
            if !agent.act_timeout_seconds.is_finite() || agent.act_timeout_seconds <= 0.0 {
                bail!("agents.human_vs_agent.act_timeout_seconds must be finite and positive");
            }
            if agent.invalid_action_limit == 0 {
                bail!("agents.human_vs_agent.invalid_action_limit must be positive");
            }
        }
        if self.agents.mode == AgentsMode::HumanVsAgent && self.agents.human_vs_agent.is_none() {
            bail!("agents.human_vs_agent is required in human_vs_agent mode");
        }
        if self.agents.mode == AgentsMode::HumanVsHuman && self.agents.human_vs_agent.is_some() {
            bail!("agents.human_vs_agent must be omitted in human_vs_human mode");
        }
        let mut consent_ids = HashSet::new();
        for item in &self.direct.consents {
            if item.id.trim().is_empty() || item.id.chars().count() > 128 {
                bail!("direct consent ids must contain 1 to 128 characters");
            }
            if !consent_ids.insert(item.id.as_str()) {
                bail!("direct consent ids must be unique");
            }
            if item.title.trim().is_empty() || item.title.chars().count() > 500 {
                bail!("direct consent titles must contain 1 to 500 characters");
            }
            if item.body.trim().is_empty() || item.body.chars().count() > 20_000 {
                bail!("direct consent bodies must contain 1 to 20000 characters");
            }
        }
        let has_information_version = !self
            .direct
            .participant_information_version
            .trim()
            .is_empty();
        let has_information_url = !self.direct.participant_information_url.trim().is_empty();
        if has_information_version != has_information_url {
            bail!("direct participant information version and URL must be configured together");
        }
        if has_information_url {
            validate_http_url(
                "direct.participant_information_url",
                &self.direct.participant_information_url,
            )?;
        }
        Ok(())
    }

    /// Lists missing runtime credentials or identifiers which prevent participant intake.
    pub fn activation_issues(&self) -> Vec<String> {
        let mut issues = Vec::new();
        if self.transcription.enabled && self.speechmatics.api_key.trim().is_empty() {
            issues.push("Speechmatics API key is required when transcription is enabled.".into());
        }
        if self.tts.enabled && self.tts.api_key.trim().is_empty() {
            issues.push("ElevenLabs API key is required when text to speech is enabled.".into());
        }
        if self.tts.enabled && self.tts.voice_id.trim().is_empty() {
            issues.push("ElevenLabs voice id is required when text to speech is enabled.".into());
        }
        issues
    }
}

/// Rejects malformed hosted-service WebSocket URLs.
pub(crate) fn validate_websocket_url(field: &str, value: &str) -> Result<()> {
    let uri: http::Uri = value
        .parse()
        .with_context(|| format!("{field} must be a valid URL"))?;
    if !matches!(uri.scheme_str(), Some("ws" | "wss")) || uri.authority().is_none() {
        bail!("{field} must use ws or wss and include a host");
    }
    Ok(())
}

/// Rejects malformed or non-HTTP URLs used in browser-visible configuration.
fn validate_http_url(field: &str, value: &str) -> Result<()> {
    let uri: http::Uri = value
        .parse()
        .with_context(|| format!("{field} must be a valid URL"))?;
    if !matches!(uri.scheme_str(), Some("http" | "https")) || uri.authority().is_none() {
        bail!("{field} must use http or https and include a host");
    }
    if uri
        .authority()
        .is_some_and(|authority| authority.as_str().contains('@'))
    {
        bail!("{field} must not contain credentials");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns a minimal valid configuration for one-field boundary mutations.
    fn valid_config() -> ExperimentConfig {
        let mut config = ExperimentConfig::default();
        config.database.url = "sqlite:///:memory:".to_string();
        config
    }

    /// Confirms invalid floating-point agent timeouts are rejected before Duration conversion.
    #[test]
    fn validation_rejects_nonfinite_agent_timeout() {
        let mut config = ExperimentConfig::default();
        config.database.url = "sqlite:///:memory:".to_string();
        config.agents.human_vs_agent = Some(HumanVsAgentConfig {
            act_timeout_seconds: f64::NAN,
            ..HumanVsAgentConfig::default()
        });

        let error = config.validate().expect_err("nonfinite timeout fails");

        assert!(error.to_string().contains("act_timeout_seconds"));
    }

    /// Confirms incomplete hosted-service drafts remain editable but cannot accept participants.
    #[test]
    fn activation_validation_requires_enabled_tts_secrets() {
        let mut config = ExperimentConfig::default();
        config.database.url = "sqlite:///:memory:".to_string();
        config.tts.enabled = true;
        config.tts.voice_id = "voice".to_string();

        config
            .validate()
            .expect("incomplete draft remains editable");
        assert!(config
            .activation_issues()
            .iter()
            .any(|issue| issue.contains("ElevenLabs API key")));
    }

    /// Confirms that consent items require no separate enablement or metadata fields.
    #[test]
    fn validation_accepts_consent_items_without_separate_enablement() {
        let mut config = ExperimentConfig::default();
        config.database.url = "sqlite:///:memory:".to_string();
        config
            .validate()
            .expect("an empty consent list disables consent");
        config.direct.consents.push(ConsentItemConfig {
            id: "study".to_string(),
            title: "Study consent".to_string(),
            body: "Agree?".to_string(),
            required: true,
        });

        config
            .validate()
            .expect("the non-empty consent list is sufficient to enable consent");
    }

    /// Covers the complete experiment-id alphabet and character-count boundaries.
    #[test]
    fn validation_checks_experiment_identifier_boundaries() {
        for accepted in ["a", "A-Z_09.test", &"x".repeat(128)] {
            let mut config = valid_config();
            config.experiment.id = Some(accepted.to_string());
            config.validate().unwrap();
        }
        for rejected in ["", "white space", "ümlaut", &"x".repeat(129)] {
            let mut config = valid_config();
            config.experiment.id = Some(rejected.to_string());
            assert!(config
                .validate()
                .unwrap_err()
                .to_string()
                .contains("experiment.id"));
        }
    }

    /// Covers exact timeout relations and all independently invalid timeout values.
    #[test]
    fn validation_checks_timeout_boundaries() {
        let mut exact = valid_config();
        exact.session.session_idle_timeout_seconds = 30;
        exact.session.session_max_lifetime_seconds = 30;
        exact.session.reconnect_grace_seconds = 0;
        exact.validate().unwrap();

        for mutate in [
            |config: &mut ExperimentConfig| config.session.waiting_session_timeout_seconds = 0,
            |config: &mut ExperimentConfig| config.session.reconnect_grace_seconds = -1,
            |config: &mut ExperimentConfig| config.session.session_idle_timeout_seconds = 0,
            |config: &mut ExperimentConfig| {
                config.session.session_max_lifetime_seconds =
                    config.session.session_idle_timeout_seconds - 1
            },
        ] {
            let mut config = valid_config();
            mutate(&mut config);
            assert!(config.validate().is_err());
        }
    }

    /// Confirms each capacity field independently rejects its zero value.
    #[test]
    fn validation_requires_every_capacity_to_be_positive() {
        for mutate in [
            |config: &mut ExperimentConfig| config.capacity.max_active_sessions = 0,
            |config: &mut ExperimentConfig| config.capacity.max_waiting_sessions = 0,
            |config: &mut ExperimentConfig| config.capacity.max_unattached_participants = 0,
            |config: &mut ExperimentConfig| config.capacity.max_transcription_streams = 0,
            |config: &mut ExperimentConfig| config.capacity.storage_reserve_megabytes = 0,
        ] {
            let mut config = valid_config();
            mutate(&mut config);
            assert!(config
                .validate()
                .unwrap_err()
                .to_string()
                .contains("capacity"));
        }
    }

    /// Confirms browser origins are host-only HTTP URLs without duplicates or credentials.
    #[test]
    fn validation_checks_origin_security_boundaries() {
        for accepted in ["http://localhost:3000", "https://example.test/"] {
            let mut config = valid_config();
            config.server.allowed_origins = vec![accepted.to_string()];
            config.validate().unwrap();
        }
        for rejected in [
            "ws://example.test",
            "https://example.test/path",
            "https://example.test/?query=1",
            "https://user:password@example.test",
        ] {
            let mut config = valid_config();
            config.server.allowed_origins = vec![rejected.to_string()];
            assert!(
                config.validate().is_err(),
                "origin {rejected:?} was accepted"
            );
        }
        let mut duplicate = valid_config();
        duplicate.server.allowed_origins = vec![
            "https://EXAMPLE.test".to_string(),
            "https://example.TEST".to_string(),
        ];
        assert!(duplicate
            .validate()
            .unwrap_err()
            .to_string()
            .contains("unique"));
    }

    /// Confirms protocol-one audio constants and jitter buffer endpoints are exact.
    #[test]
    fn validation_checks_voice_protocol_boundaries() {
        for jitter in [20, 5_000] {
            let mut config = valid_config();
            config.voice.enabled = true;
            config.voice.jitter_buffer_ms = jitter;
            config.validate().unwrap();
        }
        for jitter in [19, 5_001] {
            let mut config = valid_config();
            config.voice.jitter_buffer_ms = jitter;
            assert!(config.validate().is_err());
        }
        for mutate in [
            |config: &mut ExperimentConfig| config.voice.sample_rate_hz = 48_000,
            |config: &mut ExperimentConfig| config.voice.frame_duration_ms = 10,
        ] {
            let mut config = valid_config();
            config.voice.enabled = true;
            mutate(&mut config);
            assert!(config
                .validate()
                .unwrap_err()
                .to_string()
                .contains("protocol version 1"));
        }
    }

    /// Confirms hosted transcription validation rejects every unsupported or unsafe value.
    #[test]
    fn validation_checks_transcription_provider_and_float_boundaries() {
        let mut valid = valid_config();
        valid.transcription.enabled = true;
        valid.validate().unwrap();

        for invalid in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let mut config = valid.clone();
            config.speechmatics.max_delay = invalid;
            assert!(config.validate().is_err());
            let mut config = valid.clone();
            config.speechmatics.end_of_utterance_silence_trigger = invalid;
            assert!(config.validate().is_err());
        }
        for (field, value) in [
            ("provider", "other"),
            ("model", "unsupported"),
            ("language", ""),
            ("url", "https://example.test"),
        ] {
            let mut config = valid.clone();
            match field {
                "provider" => config.transcription.provider = value.to_string(),
                "model" => config.transcription.model = value.to_string(),
                "language" => config.transcription.language = value.to_string(),
                "url" => config.speechmatics.realtime_url = value.to_string(),
                _ => unreachable!(),
            }
            assert!(config.validate().is_err());
        }
    }

    /// Confirms agent mode/object consistency and decision limits are validated together.
    #[test]
    fn validation_checks_agent_configuration_matrix() {
        let mut missing = valid_config();
        missing.agents.mode = AgentsMode::HumanVsAgent;
        assert!(missing.validate().is_err());

        let mut unexpected = valid_config();
        unexpected.agents.human_vs_agent = Some(HumanVsAgentConfig::default());
        assert!(unexpected.validate().is_err());

        for timeout in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let mut config = valid_config();
            config.agents.mode = AgentsMode::HumanVsAgent;
            config.agents.human_vs_agent = Some(HumanVsAgentConfig {
                act_timeout_seconds: timeout,
                ..HumanVsAgentConfig::default()
            });
            assert!(config.validate().is_err());
        }
        let mut zero_limit = valid_config();
        zero_limit.agents.mode = AgentsMode::HumanVsAgent;
        zero_limit.agents.human_vs_agent = Some(HumanVsAgentConfig {
            invalid_action_limit: 0,
            ..HumanVsAgentConfig::default()
        });
        assert!(zero_limit.validate().is_err());
    }

    /// Confirms consent uniqueness, content bounds, and information-link pairing.
    #[test]
    fn validation_checks_consent_and_participant_information_boundaries() {
        let valid_item = ConsentItemConfig {
            id: "i".repeat(128),
            title: "t".repeat(500),
            body: "b".repeat(20_000),
            required: true,
        };
        let mut config = valid_config();
        config.direct.consents = vec![valid_item.clone()];
        config.validate().unwrap();

        for invalid in [
            ConsentItemConfig {
                id: " ".to_string(),
                ..valid_item.clone()
            },
            ConsentItemConfig {
                id: "i".repeat(129),
                ..valid_item.clone()
            },
            ConsentItemConfig {
                title: String::new(),
                ..valid_item.clone()
            },
            ConsentItemConfig {
                title: "t".repeat(501),
                ..valid_item.clone()
            },
            ConsentItemConfig {
                body: " ".to_string(),
                ..valid_item.clone()
            },
            ConsentItemConfig {
                body: "b".repeat(20_001),
                ..valid_item.clone()
            },
        ] {
            let mut config = valid_config();
            config.direct.consents = vec![invalid];
            assert!(config.validate().is_err());
        }
        let mut duplicate = valid_config();
        duplicate.direct.consents = vec![valid_item.clone(), valid_item];
        assert!(duplicate.validate().is_err());
        for (version, url) in [("v1", ""), ("", "https://example.test/info")] {
            let mut config = valid_config();
            config.direct.participant_information_version = version.to_string();
            config.direct.participant_information_url = url.to_string();
            assert!(config.validate().is_err());
        }
    }

    /// Confirms activation issues are complete, ordered, and whitespace-sensitive.
    #[test]
    fn activation_issues_are_stable_and_complete() {
        let mut config = valid_config();
        config.transcription.enabled = true;
        config.speechmatics.api_key = "  ".to_string();
        config.tts.enabled = true;
        config.tts.api_key = "\t".to_string();
        config.tts.voice_id = String::new();

        let issues = config.activation_issues();

        assert_eq!(issues.len(), 3);
        assert!(issues[0].contains("Speechmatics"));
        assert!(issues[1].contains("API key"));
        assert!(issues[2].contains("voice id"));
    }

    /// Confirms process identity, topology, and credentials are not accepted as experiment JSON.
    #[test]
    fn deserialization_rejects_non_experiment_fields() {
        for value in [
            serde_json::json!({"experiment": {"id": "other"}}),
            serde_json::json!({"server": {"public_base_url": "https://example.test"}}),
            serde_json::json!({"database": {"url": "sqlite://other"}}),
            serde_json::json!({"speechmatics": {"api_key": "secret"}}),
            serde_json::json!({"tts": {"api_key": "secret"}}),
        ] {
            assert!(serde_json::from_value::<ExperimentConfig>(value).is_err());
        }
    }

    /// Confirms both revisioned provider destinations are validated when enabled.
    #[test]
    fn validation_checks_revisioned_provider_endpoints() {
        let mut transcription = valid_config();
        transcription.transcription.enabled = true;
        transcription.speechmatics.realtime_url = "https://not-websocket.test".to_string();
        assert!(transcription.validate().is_err());

        let mut synthesis = valid_config();
        synthesis.tts.enabled = true;
        synthesis.tts.voice_id = "voice".to_string();
        synthesis.tts.base_url = "https://not-websocket.test".to_string();
        assert!(synthesis.validate().is_err());
    }
}
