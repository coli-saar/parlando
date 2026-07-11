use anyhow::{bail, Result};
use async_trait::async_trait;
use serde_json::json;

use crate::{
    config::ExperimentConfig,
    livekit::{create_livekit_token, livekit_identity},
    protocol::{AudioSessionPlanResponse, AudioSinkPlan},
    speechmatics::create_speechmatics_temporary_key,
};

/// Request context used to build one browser audio-session plan.
pub struct AudioSessionContext<'a> {
    /// Server experiment configuration.
    pub config: &'a ExperimentConfig,
    /// Client-facing room identifier.
    pub room_id: &'a str,
    /// Session-local player role such as `A` or `B`.
    pub role: &'a str,
    /// Client reconnect/session handle for the participant.
    pub participant_session_id: &'a str,
    /// Token lifetime for generated realtime credentials.
    pub token_ttl_seconds: i64,
}

/// Describes whether an audio-session plan establishes transcription readiness.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TranscriptionReadiness {
    /// No extra transcription readiness gate is needed for game start.
    NotRequired,
    /// Creating this plan is enough to consider transcription initialized.
    SatisfiedByPlan,
    /// A separate worker/client signal must mark transcription readiness later.
    RequiresExternalSignal,
}

/// Complete server-side result of planning a browser audio session.
pub struct PlannedAudioSession {
    /// Client-facing audio session response.
    pub response: AudioSessionPlanResponse,
    /// Readiness semantics associated with the response.
    pub transcription_readiness: TranscriptionReadiness,
}

/// LiveKit transport credentials and sink construction for partner audio.
pub trait PartnerAudioProvider: Send + Sync {
    /// Builds a LiveKit sink that publishes/subscribes browser audio for a room participant.
    fn partner_audio_sink(
        &self,
        ctx: &AudioSessionContext<'_>,
        id: &str,
        purposes: Vec<String>,
    ) -> Result<AudioSinkPlan>;
}

/// Provider-specific transcription plan that can be composed with partner audio.
pub struct TranscriptionPlan {
    /// Whether audio should be enabled for this planned provider combination.
    pub session_enabled: bool,
    /// Optional additional sink for browser-side transcription.
    pub sink: Option<AudioSinkPlan>,
    /// Whether partner audio should also be labeled as carrying transcription audio.
    pub include_transcription_in_partner_audio: bool,
    /// Readiness semantics for this transcription provider.
    pub readiness: TranscriptionReadiness,
}

/// Browser or worker transcription provider used by audio-session planning.
#[async_trait]
pub trait TranscriptionProvider: Send + Sync {
    /// Builds provider-specific transcription sinks and readiness semantics.
    async fn transcription_plan(&self, ctx: &AudioSessionContext<'_>) -> Result<TranscriptionPlan>;
}

/// Audio-session planner that composes partner audio and transcription providers.
pub struct DefaultAudioSessionPlanner;

impl DefaultAudioSessionPlanner {
    /// Builds the current client-compatible audio-session plan from typed provider pieces.
    pub async fn plan(ctx: AudioSessionContext<'_>) -> Result<PlannedAudioSession> {
        if !ctx.config.livekit.enabled {
            return Ok(PlannedAudioSession {
                response: AudioSessionPlanResponse::disabled(),
                transcription_readiness: TranscriptionReadiness::NotRequired,
            });
        }

        let livekit = LiveKitPartnerAudioProvider;
        let transcription = transcription_provider(ctx.config)?;
        let transcription_plan = transcription.transcription_plan(&ctx).await?;
        if !transcription_plan.session_enabled {
            return Ok(PlannedAudioSession {
                response: AudioSessionPlanResponse::disabled(),
                transcription_readiness: transcription_plan.readiness,
            });
        }
        let mut partner_purposes = vec!["partner-audio".to_string()];
        if transcription_plan.include_transcription_in_partner_audio {
            partner_purposes.push("transcription".to_string());
        }
        let partner_sink_id = if transcription_plan.sink.is_some() {
            "livekit-partner"
        } else {
            "livekit-combined"
        };
        let mut sinks =
            vec![livekit.partner_audio_sink(&ctx, partner_sink_id, partner_purposes)?];
        if let Some(sink) = transcription_plan.sink {
            sinks.push(sink);
        }
        Ok(PlannedAudioSession {
            response: AudioSessionPlanResponse {
                enabled: true,
                capture: json!({"audio": true}),
                sinks,
            },
            transcription_readiness: transcription_plan.readiness,
        })
    }
}

struct LiveKitPartnerAudioProvider;

impl PartnerAudioProvider for LiveKitPartnerAudioProvider {
    fn partner_audio_sink(
        &self,
        ctx: &AudioSessionContext<'_>,
        id: &str,
        purposes: Vec<String>,
    ) -> Result<AudioSinkPlan> {
        let token = create_livekit_token(
            &ctx.config.livekit,
            ctx.room_id,
            ctx.role,
            ctx.participant_session_id,
            ctx.token_ttl_seconds,
        )?;
        let identity = livekit_identity(ctx.room_id, ctx.role, ctx.participant_session_id);
        Ok(AudioSinkPlan {
            id: id.to_string(),
            provider: "livekit".to_string(),
            purposes,
            transport: "webrtc-room".to_string(),
            credentials: json!({
                "enabled": true,
                "url": ctx.config.livekit.url,
                "token": token,
                "identity": identity,
            }),
        })
    }
}

struct NoopTranscriptionProvider;

#[async_trait]
impl TranscriptionProvider for NoopTranscriptionProvider {
    async fn transcription_plan(
        &self,
        _ctx: &AudioSessionContext<'_>,
    ) -> Result<TranscriptionPlan> {
        Ok(TranscriptionPlan {
            session_enabled: true,
            sink: None,
            include_transcription_in_partner_audio: true,
            readiness: TranscriptionReadiness::NotRequired,
        })
    }
}

struct SpeechmaticsBrowserTranscriptionProvider;

#[async_trait]
impl TranscriptionProvider for SpeechmaticsBrowserTranscriptionProvider {
    async fn transcription_plan(&self, ctx: &AudioSessionContext<'_>) -> Result<TranscriptionPlan> {
        if !ctx.config.speechmatics.enabled || ctx.config.speechmatics.api_key.is_empty() {
            return Ok(TranscriptionPlan {
                session_enabled: false,
                sink: None,
                include_transcription_in_partner_audio: false,
                readiness: TranscriptionReadiness::RequiresExternalSignal,
            });
        }
        let key = create_speechmatics_temporary_key(&ctx.config.speechmatics).await?;
        Ok(TranscriptionPlan {
            session_enabled: true,
            sink: Some(AudioSinkPlan {
                id: "speechmatics-transcription".to_string(),
                provider: "speechmatics".to_string(),
                purposes: vec!["transcription".to_string()],
                transport: "websocket-stt".to_string(),
                credentials: json!({
                    "enabled": true,
                    "realtime_url": ctx.config.speechmatics.realtime_url,
                    "temporary_key": key,
                    "language": ctx.config.transcription.language,
                    "model": ctx.config.transcription.model,
                    "max_delay": ctx.config.speechmatics.max_delay,
                    "enable_partials": ctx.config.speechmatics.enable_partials,
                    "end_of_utterance_silence_trigger": ctx.config.speechmatics.end_of_utterance_silence_trigger,
                    "ttl_seconds": ctx.config.speechmatics.temporary_key_ttl_seconds,
                }),
            }),
            include_transcription_in_partner_audio: false,
            readiness: TranscriptionReadiness::SatisfiedByPlan,
        })
    }
}

struct LiveKitWorkerTranscriptionProvider;

#[async_trait]
impl TranscriptionProvider for LiveKitWorkerTranscriptionProvider {
    async fn transcription_plan(
        &self,
        _ctx: &AudioSessionContext<'_>,
    ) -> Result<TranscriptionPlan> {
        Ok(TranscriptionPlan {
            session_enabled: true,
            sink: None,
            include_transcription_in_partner_audio: true,
            readiness: TranscriptionReadiness::RequiresExternalSignal,
        })
    }
}

/// Selects the configured transcription provider implementation.
fn transcription_provider(config: &ExperimentConfig) -> Result<Box<dyn TranscriptionProvider>> {
    if !config.transcription.enabled {
        return Ok(Box::new(NoopTranscriptionProvider));
    }
    match config.transcription.provider.as_str() {
        "speechmatics" => Ok(Box::new(SpeechmaticsBrowserTranscriptionProvider)),
        "livekit" => Ok(Box::new(LiveKitWorkerTranscriptionProvider)),
        provider => bail!("unsupported transcription provider: {provider}"),
    }
}

#[cfg(test)]
mod tests {
    use axum::{routing::post, Json, Router};
    use serde_json::{json, Value};

    use super::*;
    use crate::config::SpeechmaticsConfig;

    /// Starts a local Speechmatics-like management endpoint for planner tests.
    async fn speechmatics_key_server(key: &'static str) -> String {
        let app = Router::new().route(
            "/v1/api_keys",
            post(move || async move { Json(json!({"key_value": key})) }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://{address}/v1/api_keys")
    }

    /// Builds a minimal config with LiveKit credentials for audio planner tests.
    fn livekit_config() -> ExperimentConfig {
        let mut config = ExperimentConfig::default();
        config.livekit.enabled = true;
        config.livekit.url = "wss://livekit.example.test".to_string();
        config.livekit.api_key = "livekit-key".to_string();
        config.livekit.api_secret = "livekit-secret".to_string();
        config
    }

    #[tokio::test]
    async fn planner_returns_disabled_response_without_livekit() {
        let config = ExperimentConfig::default();
        let planned = DefaultAudioSessionPlanner::plan(AudioSessionContext {
            config: &config,
            room_id: "room",
            role: "A",
            participant_session_id: "participant",
            token_ttl_seconds: 3600,
        })
        .await
        .unwrap();

        assert!(!planned.response.enabled);
        assert_eq!(
            planned.transcription_readiness,
            TranscriptionReadiness::NotRequired
        );
    }

    #[tokio::test]
    async fn planner_composes_livekit_combined_sink_for_livekit_transcription() {
        let mut config = livekit_config();
        config.transcription.enabled = true;
        config.transcription.provider = "livekit".to_string();
        let planned = DefaultAudioSessionPlanner::plan(AudioSessionContext {
            config: &config,
            room_id: "room",
            role: "A",
            participant_session_id: "participant",
            token_ttl_seconds: 3600,
        })
        .await
        .unwrap();

        assert!(planned.response.enabled);
        assert_eq!(planned.response.sinks.len(), 1);
        let sink = &planned.response.sinks[0];
        assert_eq!(sink.id, "livekit-combined");
        assert_eq!(sink.provider, "livekit");
        assert_eq!(sink.purposes, vec!["partner-audio", "transcription"]);
        assert_eq!(
            planned.transcription_readiness,
            TranscriptionReadiness::RequiresExternalSignal
        );
    }

    #[tokio::test]
    async fn planner_composes_split_sinks_for_speechmatics_transcription() {
        let mut config = livekit_config();
        config.transcription.enabled = true;
        config.transcription.provider = "speechmatics".to_string();
        config.transcription.language = "de".to_string();
        config.transcription.model = "enhanced".to_string();
        config.speechmatics = SpeechmaticsConfig {
            enabled: true,
            api_key: "permanent-key".to_string(),
            realtime_url: "wss://speechmatics.example.test/v2".to_string(),
            management_url: speechmatics_key_server("temporary-key").await,
            ..SpeechmaticsConfig::default()
        };

        let planned = DefaultAudioSessionPlanner::plan(AudioSessionContext {
            config: &config,
            room_id: "room",
            role: "B",
            participant_session_id: "participant",
            token_ttl_seconds: 3600,
        })
        .await
        .unwrap();

        assert!(planned.response.enabled);
        assert_eq!(planned.response.sinks.len(), 2);
        assert_eq!(planned.response.sinks[0].id, "livekit-partner");
        assert_eq!(planned.response.sinks[0].purposes, vec!["partner-audio"]);
        assert_eq!(planned.response.sinks[1].id, "speechmatics-transcription");
        assert_eq!(planned.response.sinks[1].provider, "speechmatics");
        assert_eq!(
            planned.response.sinks[1].credentials["temporary_key"],
            Value::String("temporary-key".to_string())
        );
        assert_eq!(
            planned.transcription_readiness,
            TranscriptionReadiness::SatisfiedByPlan
        );
    }
}
