//! Authenticated audio transport and transcription stream lifecycle.
//!
//! A participant obtains an audio plan and presents its room-bound upgrade ticket. The
//! socket loop relays audio through the session audio registry and passes provider
//! readiness and final transcripts to the readiness and conversation units. Session
//! lifecycle controls the interval in which audio may flow, including the farewell
//! period.

use super::{
    authenticated_participant_id, commit_final_transcript, json, mark_participant_audio_ready,
    maybe_start_game, now_iso, participant_role, publish_transcription_readiness,
    session_accepts_audio, session_is_active, session_transition_lock, touch_transport_activity,
    validate_websocket_origin, voice_message, websocket_path, AppError, AppState, Arc, AudioFrame,
    AudioOutbound, AudioSessionPlanResponse, AudioSessionRequest, ConnectionLiveness, Deserialize,
    Duration, Extension, FuturesStreamExt, HeaderMap, Instant, Json, Message, Mutex, OnceLock,
    ParticipantPrincipal, Path, Query, Response, Result, SinkExt, State, TranscriptionEvent,
    TranscriptionInput, TranscriptionSessionContext, UpgradePurpose, UpgradeTicketClaims,
    WebSocket, WebSocketUpgrade, AUDIO_CHANNELS, AUDIO_FRAME_DURATION_MS, AUDIO_PROTOCOL_VERSION,
    AUDIO_SAMPLE_RATE,
};

use super::readiness::{
    clear_forming_audio_ready, report_transcription_failure, TranscriptionFailure,
};

/// Startup budget covers connecting and receiving the provider's readiness acknowledgement.
const TRANSCRIPTION_START_TIMEOUT: Duration = Duration::from_secs(10);

/// Issues the participant's audio connection plan for this session.
pub(super) async fn audio_session(
    State(state): State<Arc<AppState>>,
    Path(public_session_id): Path<String>,
    principal: Option<Extension<ParticipantPrincipal>>,
    Json(_request): Json<AudioSessionRequest>,
) -> Result<Json<AudioSessionPlanResponse>, AppError> {
    let participant_session_id = authenticated_participant_id(principal.clone())?;
    let role = participant_role(&state, &public_session_id, &participant_session_id).await?;
    if !state.context.config.is_speech() {
        return Ok(Json(AudioSessionPlanResponse::disabled()));
    }
    if !session_accepts_audio(&state, &public_session_id).await {
        return Err(AppError::bad_request("Voice chat is no longer available."));
    }
    let claims = UpgradeTicketClaims {
        public_session_id: public_session_id.clone(),
        participant_session_id,
        role: role.as_str().to_string(),
        generation: principal.map_or(1, |Extension(principal)| principal.generation),
        purpose: UpgradePurpose::Audio,
        expires_at: 0,
    };
    let token = state.upgrade_tickets.issue(claims).await;
    Ok(Json(AudioSessionPlanResponse {
        enabled: true,
        websocket_url: Some(websocket_path(
            &state.context.config,
            &format!("ws/audio/{public_session_id}"),
        )),
        token: Some(token),
        protocol_version: AUDIO_PROTOCOL_VERSION,
        sample_rate_hz: AUDIO_SAMPLE_RATE,
        channels: AUDIO_CHANNELS,
        frame_duration_ms: AUDIO_FRAME_DURATION_MS,
        jitter_buffer_ms: state.context.config.voice.jitter_buffer_ms,
    }))
}

#[derive(Deserialize)]
pub(super) struct AudioSocketQuery {
    token: String,
}

/// Authenticates and upgrades one participant-owned audio transport.
pub(super) async fn audio_socket(
    State(state): State<Arc<AppState>>,
    Path(public_session_id): Path<String>,
    Query(query): Query<AudioSocketQuery>,
    headers: HeaderMap,
    upgrade_lease: Option<Extension<Arc<crate::deployment::UpgradeLease>>>,
    ws: WebSocketUpgrade,
) -> Result<Response, AppError> {
    if !state.context.config.is_speech() {
        return Err(AppError::forbidden("Voice chat is disabled."));
    }
    validate_websocket_origin(&state.context.config, &headers)?;
    let claims = state
        .upgrade_tickets
        .consume(&query.token, UpgradePurpose::Audio, &public_session_id)
        .await
        .ok_or_else(|| AppError::forbidden("Invalid or expired audio token"))?;
    if !state
        .participant_auth
        .generation_is_active(
            &claims.participant_session_id,
            claims.generation,
            state.mount_id,
        )
        .await
    {
        #[cfg(not(test))]
        return Err(AppError::forbidden("Audio ticket credential was revoked"));
    }
    let role = participant_role(&state, &public_session_id, &claims.participant_session_id).await?;
    if role.as_str() != claims.role {
        return Err(AppError::forbidden(
            "Audio token role no longer matches participant",
        ));
    }
    Ok(ws
        .max_frame_size(8 * 1024)
        .max_message_size(8 * 1024)
        .on_upgrade(move |socket| async move {
            // The host must see this connection while Axum completes the upgrade.
            let _upgrade_lease = upgrade_lease;
            audio_websocket_loop(state, socket, claims).await;
        }))
}

/// Connects one provider stream and waits for its explicit readiness acknowledgement.
async fn start_transcription(
    state: &AppState,
    role: &str,
) -> Result<Option<crate::transcription::TranscriptionSessionHandle>> {
    let Some(provider) = state.transcription_provider.as_ref() else {
        if super::speechmatics_readiness_required(&state.context.config) {
            anyhow::bail!("required transcription provider is unavailable");
        }
        return Ok(None);
    };
    let mut session = provider
        .start_session(TranscriptionSessionContext {
            role: role.to_string(),
            language: state.context.config.transcription.language.clone(),
            model: state.context.config.transcription.model.clone(),
        })
        .await?;
    while let Some(event) = session.events.recv().await {
        match event {
            TranscriptionEvent::Ready => return Ok(Some(session)),
            TranscriptionEvent::Failed(error) => anyhow::bail!("{error}"),
            TranscriptionEvent::FinalUtterance(_) => {}
        }
    }
    anyhow::bail!("transcription stream closed before becoming ready")
}

/// Relays browser PCM and consumes normalized server-side transcription events.
async fn audio_websocket_loop(
    state: Arc<AppState>,
    socket: WebSocket,
    claims: UpgradeTicketClaims,
) {
    let public_session_id = claims.public_session_id.clone();
    let role = claims.role.clone();
    let participant_session_id = claims.participant_session_id.clone();
    let connection_key = format!("{public_session_id}:{role}");
    let Some(transition_lock) = session_transition_lock(&state, &public_session_id).await else {
        return;
    };
    let transition_guard = transition_lock.lock().await;
    if !session_accepts_audio(&state, &public_session_id).await {
        return;
    }
    let liveness = Arc::new(ConnectionLiveness::new());
    let (connection_generation, mut shutdown, replaced) = state
        .audio_connections
        .register(connection_key.clone(), liveness.clone())
        .await;
    if replaced {
        state.context.installation.telemetry.record_reconnection();
    }
    let (generation, mut outbound) = state
        .audio_sessions
        .connect(&public_session_id, &role)
        .await;
    clear_forming_audio_ready(&state, &public_session_id, &participant_session_id).await;
    drop(transition_guard);
    let (mut sender, mut incoming) = socket.split();
    let send_task = tokio::spawn(async move {
        while let Some(outbound) = outbound.recv().await {
            let message = match outbound {
                AudioOutbound::Binary(bytes) => Message::Binary(bytes),
                AudioOutbound::Text(text) => Message::Text(text),
            };
            if sender.send(message).await.is_err() {
                break;
            }
        }
    });

    // Relay browser audio independently while the per-socket provider starts.
    let transcription_input = Arc::new(OnceLock::new());
    let transcription_stream_started_at = Arc::new(OnceLock::<String>::new());
    let transcription_clock = Arc::new(Mutex::new(
        crate::transcription::clock::TranscriptionClock::default(),
    ));
    let mut event_task = {
        let state = state.clone();
        let public_session_id = public_session_id.clone();
        let role = role.clone();
        let participant_session_id = participant_session_id.clone();
        let transcription_input = transcription_input.clone();
        let transcription_stream_started_at = transcription_stream_started_at.clone();
        let transcription_clock = transcription_clock.clone();
        let generation = generation.clone();
        let transition_lock = transition_lock.clone();
        tokio::spawn(async move {
            let startup = tokio::time::timeout(
                TRANSCRIPTION_START_TIMEOUT,
                start_transcription(&state, &role),
            )
            .await;
            let result = match startup {
                Ok(result) => result.map_err(|error| {
                    tracing::warn!(%error, %public_session_id, %role, "could not start transcription session");
                    TranscriptionFailure::Unavailable
                }),
                Err(_) => {
                    tracing::warn!(%public_session_id, %role, "timed out starting transcription session");
                    Err(TranscriptionFailure::Timeout)
                }
            };
            let mut transcription = match result {
                Ok(session) => session,
                Err(failure) => {
                    report_transcription_failure(
                        &state,
                        &public_session_id,
                        &participant_session_id,
                        &role,
                        &generation,
                        failure,
                    )
                    .await;
                    return;
                }
            };
            if transcription.is_some() || !state.context.config.transcription_active() {
                {
                    let _guard = transition_lock.lock().await;
                    if !state
                        .audio_sessions
                        .is_current(&public_session_id, &role, &generation)
                        .await
                    {
                        return;
                    }
                    if let Some(session) = &transcription {
                        let _ = transcription_input.set(session.input.clone());
                    }
                    mark_participant_audio_ready(
                        &state,
                        &public_session_id,
                        &participant_session_id,
                    )
                    .await;
                }
                let _ = state
                    .session_bus(&public_session_id)
                    .await
                    .send(voice_message(&state, &public_session_id).await);
                maybe_start_game(state.clone(), &public_session_id).await;
                publish_transcription_readiness(&state, &public_session_id, &role).await;
            }
            let Some(session) = &mut transcription else {
                return;
            };
            while let Some(event) = session.events.recv().await {
                match event {
                    TranscriptionEvent::Ready => {}
                    TranscriptionEvent::FinalUtterance(utterance) => {
                        let mut clock = transcription_clock.lock().await;
                        let provider_start = utterance.start_time_ms;
                        let utterance = match clock.map(utterance) {
                            Ok(utterance) => utterance,
                            Err(error) => {
                                tracing::warn!(%error, %public_session_id, "invalid transcript clock correspondence");
                                continue;
                            }
                        };
                        clock.discard_before(provider_start);
                        drop(clock);
                        let Some(stream_started_at) = transcription_stream_started_at.get() else {
                            tracing::warn!(%public_session_id, "transcription result arrived without an audio-stream clock origin");
                            continue;
                        };
                        if let Err(error) = commit_final_transcript(
                            &state,
                            &public_session_id,
                            &participant_session_id,
                            stream_started_at,
                            utterance,
                        )
                        .await
                        {
                            tracing::warn!(error = %error.message, %public_session_id, "failed to commit final transcript");
                        }
                    }
                    TranscriptionEvent::Failed(error) => {
                        tracing::warn!(%error, %public_session_id, %role, "transcription session failed");
                        break;
                    }
                }
            }
            report_transcription_failure(
                &state,
                &public_session_id,
                &participant_session_id,
                &role,
                &generation,
                TranscriptionFailure::StreamFailed,
            )
            .await;
        })
    };
    let mut last_activity_touch = Instant::now();
    let media_started = Instant::now();
    let mut first_timestamp_ms = None;
    let mut last_sequence = None;
    let mut last_timestamp_ms = None;
    let mut dropped_audio_frames = 0_u64;
    loop {
        let next_message = tokio::select! {
            _ = shutdown.recv() => break,
            message = incoming.next() => message,
        };
        let Some(Ok(message)) = next_message else {
            break;
        };
        match message {
            Message::Binary(bytes) => match AudioFrame::decode(&bytes) {
                Ok(frame) => {
                    liveness.touch_message(true);
                    let stream_origin = *first_timestamp_ms.get_or_insert(frame.timestamp_ms);
                    let media_elapsed_ms = frame.timestamp_ms.saturating_sub(stream_origin);
                    let wall_elapsed_ms = media_started.elapsed().as_millis() as u64;
                    let timing_valid = last_sequence
                        .is_none_or(|sequence| frame.sequence > sequence)
                        && last_timestamp_ms
                            .is_none_or(|timestamp| frame.timestamp_ms >= timestamp)
                        && media_elapsed_ms
                            <= wall_elapsed_ms.saturating_mul(2).saturating_add(5_000);
                    if !timing_valid {
                        dropped_audio_frames += 1;
                        state
                            .context
                            .installation
                            .telemetry
                            .record_audio_frame_dropped();
                        continue;
                    }
                    last_sequence = Some(frame.sequence);
                    last_timestamp_ms = Some(frame.timestamp_ms);
                    if last_activity_touch.elapsed() >= Duration::from_secs(30) {
                        touch_transport_activity(&state, &public_session_id).await;
                        last_activity_touch = Instant::now();
                    }
                    if !state
                        .audio_sessions
                        .is_current(&public_session_id, &role, &generation)
                        .await
                    {
                        break;
                    }
                    if !session_accepts_audio(&state, &public_session_id).await {
                        continue;
                    }
                    state
                        .audio_sessions
                        .relay_partner(&public_session_id, &role, bytes)
                        .await;
                    state.context.installation.telemetry.record_audio_frame();
                    if let Some(input) = transcription_input.get() {
                        if session_is_active(&state, &public_session_id).await {
                            let received_at = now_iso();
                            let timestamp_ms = frame.timestamp_ms;
                            let mut clock = transcription_clock.lock().await;
                            match input.try_send(TranscriptionInput::Audio(frame)) {
                                Ok(()) => {
                                    let _ = transcription_stream_started_at.set(received_at);
                                    if let Err(error) = clock.record(timestamp_ms) {
                                        tracing::warn!(%error, "failed to record transcription clock");
                                    }
                                }
                                Err(_) => {
                                    drop(clock);
                                    state
                                        .context
                                        .installation
                                        .telemetry
                                        .record_asr_backpressure();
                                    state.audio_sessions.send_control(&public_session_id, &role, json!({"type":"transcriptionStatus","ready":false,"message":"ASR is falling behind"}).to_string()).await;
                                }
                            }
                        }
                    }
                }
                Err(error) => {
                    dropped_audio_frames += 1;
                    state
                        .context
                        .installation
                        .telemetry
                        .record_audio_frame_dropped();
                    tracing::debug!(%error, %public_session_id, "dropped invalid browser audio frame")
                }
            },
            Message::Close(_) => break,
            _ => {}
        }
    }
    {
        let _guard = transition_lock.lock().await;
        if state
            .audio_sessions
            .is_current(&public_session_id, &role, &generation)
            .await
        {
            clear_forming_audio_ready(&state, &public_session_id, &participant_session_id).await;
        }
        state
            .audio_sessions
            .disconnect(&public_session_id, &role, &generation)
            .await;
        state
            .audio_connections
            .unregister(&connection_key, &connection_generation)
            .await;
    }
    // Release the browser transport before allowing the provider to drain final results.
    send_task.abort();
    drop(incoming);
    if let Some(input) = transcription_input.get() {
        let _ = tokio::time::timeout(
            Duration::from_secs(2),
            input.send(TranscriptionInput::Finish),
        )
        .await;
    }
    if dropped_audio_frames > 0 {
        tracing::debug!(dropped_audio_frames, %public_session_id, "browser audio frames were dropped");
    }
    // An unacknowledged provider has no input to finish. Cancel its pending startup now.
    if transcription_input.get().is_none()
        || tokio::time::timeout(Duration::from_secs(5), &mut event_task)
            .await
            .is_err()
    {
        event_task.abort();
    }
}
