import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { AudioSessionController } from "./audio/audioSessionController.js";
import { ParlandoAudioSink } from "./audio/parlandoAudioSink.js";
import { MicrophoneSource } from "./audio/microphoneSource.js";
import {
  initialVoicePreflight,
  initialVoiceStatus,
  type AudioSessionContext,
  type VoicePreflight,
  type VoiceStatus
} from "./audio/types.js";
import { experimentAllowsIntake, requiredConsentsAccepted } from "./helpers.js";
import { MicrophoneLevelMeter, TranscriptionProgress } from "./voiceComponents.js";
import {
  ParticipantClient,
  decodeServerMessage,
  reduceParticipantState,
  type ExperimentInfo,
  type ParticipantState,
  type ParticipantOutcome,
  type RecruitmentHandoff,
  type PlayerMessage,
  type PlayerRole,
  type Presence,
  playerMessage,
  type ServerMessage
} from "./protocol.js";

export interface GameSession<TObservation, TAction, TCompletion = Record<string, unknown>> {
  sessionId: string;
  role: PlayerRole;
  observation: TObservation;
  /** Most recent accepted action, or null for an initial or resynchronized observation. */
  transition: GameTransition<TAction> | null;
  availableActions: TAction[] | null;
  conversation: PlayerMessage[];
  presence: Presence;
  voiceStatus: VoiceStatus;
  voicePreflight: VoicePreflight;
  voiceEnabled: boolean;
  connected: boolean;
  interactionEnabled: boolean;
  sendAction(action: TAction): void;
  sendMessage(text: string): void;
  setMicrophoneMuted(muted: boolean): Promise<void>;
  leave(): void;
}

/** One accepted action made observable to both player roles. */
export interface GameTransition<TAction> {
  actor: PlayerRole;
  action: TAction;
}

export interface ParticipantAppProps<TObservation, TAction, TCompletion = Record<string, unknown>> {
  renderGame(session: GameSession<TObservation, TAction, TCompletion>): ReactNode;
  /** Renders game-specific public success content inside the standard terminal shell. */
  renderCompletion?(completion: TCompletion): ReactNode;
  baseUrl?: string;
}

interface ParticipantAppRuntimeProps<TObservation, TAction, TCompletion = Record<string, unknown>> {
  renderGame(session: GameSession<TObservation, TAction, TCompletion>): ReactNode;
  renderCompletion?(completion: TCompletion): ReactNode;
  apiClient: ParticipantClient;
  createAudioController: () => AudioSessionController;
}

interface LiveSession<TObservation, TAction, TCompletion = Record<string, unknown>> {
  participantState: ParticipantState<TObservation, TAction, TCompletion>;
  transition: GameTransition<TAction> | null;
  socket: WebSocket;
  synchronization: "connecting" | "connected" | "reconnecting";
  conversation: PlayerMessage[];
}

/** @internal Minimal channel state used by source-level tests. */
export interface GameInputSession {
  socket: WebSocket;
  participantState: { state: string };
  synchronization: "connecting" | "connected" | "reconnecting";
}

/** @internal Game-channel heartbeat cadence; heartbeats are never research activity. */
export const CLIENT_HEARTBEAT_INTERVAL_MS = 1_000;

/** @internal Returns the state update applied when the server announces game completion. */
export function completedSessionPatch<TCompletion>(completion: TCompletion | undefined): {
  completed: true;
  completion: TCompletion | null;
} {
  return { completed: true, completion: completion ?? null };
}

/** @internal Returns whether participant game-channel messages should still be sent. */
export function canSendGameMessage(session: Pick<GameInputSession, "participantState" | "synchronization"> | null): boolean {
  return Boolean(session?.participantState.state === "active" && session.synchronization === "connected");
}

/** @internal Sends an action only while the reusable session is still accepting game input. */
export function sendActionIfGameActive<TAction>(
  apiClient: Pick<ParticipantClient, "sendAction">,
  session: GameInputSession | null,
  action: TAction
): void {
  if (!session || !canSendGameMessage(session)) return;
  apiClient.sendAction(session.socket, action);
}

/** @internal Sends a chat message only while the reusable session is still accepting game input. */
export function sendMessageIfGameActive(
  apiClient: Pick<ParticipantClient, "sendMessage">,
  session: GameInputSession | null,
  text: string
): void {
  if (!session || !canSendGameMessage(session)) return;
  apiClient.sendMessage(session.socket, text);
}

/** Runs the standard participant lifecycle before rendering one game session. */
export function ParticipantApp<
  TObservation,
  TAction = unknown,
  TCompletion = Record<string, unknown>
>({ renderGame, renderCompletion, baseUrl }: ParticipantAppProps<TObservation, TAction, TCompletion>) {
  const apiClient = useMemo(() => new ParticipantClient({ baseUrl }), [baseUrl]);
  return (
    <ParticipantAppRuntime
      apiClient={apiClient}
      createAudioController={createDefaultAudioController}
      renderGame={renderGame}
      renderCompletion={renderCompletion}
    />
  );
}

/** @internal Injects deterministic runtime dependencies for source-level tests only. */
export function ParticipantAppTestHarness<
  TObservation,
  TAction = unknown,
  TCompletion = Record<string, unknown>
>(props: ParticipantAppRuntimeProps<TObservation, TAction, TCompletion>) {
  return <ParticipantAppRuntime {...props} />;
}

/** Owns the participant lifecycle using selected transport dependencies. */
function ParticipantAppRuntime<
  TObservation,
  TAction = unknown,
  TCompletion = Record<string, unknown>
>({ renderGame, renderCompletion, apiClient, createAudioController }: ParticipantAppRuntimeProps<TObservation, TAction, TCompletion>) {
  const audioControllerRef = useRef<AudioSessionController | null>(null);
  if (!audioControllerRef.current) audioControllerRef.current = createAudioController();
  const audioController = audioControllerRef.current;

  const [publicConfig, setPublicConfig] = useState<ExperimentInfo | null>(null);
  const [configLoading, setConfigLoading] = useState(true);
  const [consentDecisions, setConsentDecisions] = useState<Record<string, boolean>>({});
  const [session, setSession] = useState<LiveSession<TObservation, TAction, TCompletion> | null>(null);
  const [error, setError] = useState("");
  const [audioInputs, setAudioInputs] = useState<MediaDeviceInfo[]>([]);
  const [selectedAudioInputId, setSelectedAudioInputId] = useState("");
  const [status, setVoiceStatus] = useState<VoiceStatus>(initialVoiceStatus);
  const [voicePreflight, setVoicePreflight] = useState<VoicePreflight>(initialVoicePreflight);
  const [voiceReconnectGeneration, setVoiceReconnectGeneration] = useState(0);
  const [entering, setEntering] = useState(false);
  const sessionRef = useRef<LiveSession<TObservation, TAction, TCompletion> | null>(null);
  const connectSessionRef = useRef<(state: ParticipantState<TObservation, TAction, TCompletion>) => Promise<void>>(async () => {});
  const scheduleGameReconnectRef = useRef<(state: ParticipantState<TObservation, TAction, TCompletion>) => void>(() => {});
  const reconnectEnabledRef = useRef(false);
  const reconnectStartedAtRef = useRef(0);
  const reconnectAttemptsRef = useRef(0);
  const reconnectTimerRef = useRef<number | null>(null);
  const voicePreparationGenerationRef = useRef(0);
  const consentReady = requiredConsentsAccepted(publicConfig, consentDecisions);
  const enabled = isVoiceEnabled(publicConfig);
  const canEnter = Boolean(
    !entering && experimentAllowsIntake(publicConfig?.status) && consentReady && (!enabled || voicePreflight.ready)
  );

  const refreshAudioInputs = useCallback(async () => {
    if (!navigator.mediaDevices?.enumerateDevices) return [];
    const devices = await navigator.mediaDevices.enumerateDevices();
    const inputs = devices.filter((device) => device.kind === "audioinput");
    setAudioInputs(inputs);
    return inputs;
  }, []);

  const endCurrentSession = useCallback(() => {
    voicePreparationGenerationRef.current += 1;
    reconnectEnabledRef.current = false;
    if (reconnectTimerRef.current !== null) window.clearTimeout(reconnectTimerRef.current);
    reconnectTimerRef.current = null;
    void audioController.disconnect(true);
    closeSessionSocket(sessionRef.current);
  }, [audioController]);

  /** Commits an explicit leave over HTTP, then closes transports after receiving the terminal state. */
  const leave = useCallback(() => {
    const current = sessionRef.current;
    if (!current || current.participantState.state === "registered" || current.participantState.state === "ended") return;
    const sessionId = current.participantState.public_session_id;
    reconnectEnabledRef.current = false;
    if (reconnectTimerRef.current !== null) window.clearTimeout(reconnectTimerRef.current);
    reconnectTimerRef.current = null;
    void audioController.disconnect(true);
    setError("");
    void apiClient.leaveSession<TObservation, TAction, TCompletion>(sessionId).then((participantState) => {
      setSession((current) => {
        if (!current || current.participantState.state === "registered" || current.participantState.public_session_id !== sessionId) return current;
        const next = { ...current, participantState };
        sessionRef.current = next;
        closeSessionSocket(next);
        return next;
      });
    }).catch((caught) => setError(errorMessage(caught, "Could not record that you left the session.")));
  }, [apiClient, audioController]);

  const scheduleGameReconnect = useCallback(
    (participantState: ParticipantState<TObservation, TAction, TCompletion>) => {
      const current = sessionRef.current;
      if (!reconnectEnabledRef.current || !current || current.participantState.state === "ended" || reconnectTimerRef.current !== null) return;
      if (reconnectStartedAtRef.current === 0) reconnectStartedAtRef.current = Date.now();
      if (Date.now() - reconnectStartedAtRef.current >= 15_000) {
        setError("The connection could not be restored in time.");
        return;
      }
      const delays = [1_000, 2_000, 5_000, 10_000];
      const remaining = 15_000 - (Date.now() - reconnectStartedAtRef.current);
      const delay = Math.min(delays[Math.min(reconnectAttemptsRef.current, delays.length - 1)], remaining);
      reconnectAttemptsRef.current += 1;
      reconnectTimerRef.current = window.setTimeout(() => {
        reconnectTimerRef.current = null;
        void connectSessionRef.current(participantState).catch((caught) => {
          setError(errorMessage(caught, "Could not reconnect to the game channel."));
          scheduleGameReconnectRef.current(participantState);
        });
      }, delay);
    },
    []
  );
  scheduleGameReconnectRef.current = scheduleGameReconnect;

  const connectSession = useCallback(
    async (participantState: ParticipantState<TObservation, TAction, TCompletion>) => {
      if (participantState.state === "registered" || participantState.state === "ended") {
        if (participantState.state === "ended") {
          setSession((current) => current ? { ...current, participantState } : current);
        }
        return;
      }
      const sessionId = participantState.public_session_id;
      const gameSession = await apiClient.getGameSession(sessionId);
      const socket = new WebSocket(apiClient.socketUrl(gameSession));
      setSession((current) => {
        const currentId = current?.participantState.state === "registered" ? null : current?.participantState.public_session_id;
        const next = currentId === sessionId
          ? { ...current!, participantState, socket, synchronization: "connecting" as const }
          : {
            participantState,
            transition: null,
            socket,
            synchronization: "connecting" as const,
            conversation: []
            };
        sessionRef.current = next;
        return next;
      });

      socket.addEventListener("open", () => {
        if (sessionRef.current?.socket !== socket) return;
        reconnectStartedAtRef.current = 0;
        reconnectAttemptsRef.current = 0;
        setError("");
        socket.send(JSON.stringify({ type: "ready" }));
        setSession((current) => (current?.socket === socket ? { ...current, synchronization: "connected" } : current));
      });
      socket.addEventListener("error", () => {
        if (sessionRef.current?.socket !== socket) return;
        setError("Could not connect to the game channel. Retrying…");
      });
      socket.addEventListener("message", (event) => {
        if (sessionRef.current?.socket !== socket) return;
        let message: ServerMessage<TObservation, TAction, TCompletion>;
        try {
          if (typeof event.data !== "string") throw new Error("non-text message");
          message = decodeServerMessage<TObservation, TAction, TCompletion>(JSON.parse(event.data));
        } catch {
          setError("The server sent an invalid game message.");
          socket.close(1002, "Invalid server message");
          return;
        }
        if (message.type === "participant_state") {
          setSession((current) =>
            current?.socket === socket
              ? {
                  ...current,
                  participantState: reduceParticipantState(current.participantState, message.participant_state),
                  transition: null,
                  synchronization: "connected"
                }
              : current
          );
          if (message.participant_state.state === "ended") {
            reconnectEnabledRef.current = false;
            void audioController.disconnect(true);
            socket.close();
          }
          return;
        }
        if (message.type === "transition") {
          setSession((current) =>
            current?.socket === socket
              ? {
                  ...current,
                  transition: { actor: message.actor, action: message.action },
                  participantState: current.participantState.state === "active"
                    ? { ...current.participantState, observation: message.observation, available_actions: message.available_actions }
                    : current.participantState
                }
              : current
          );
          return;
        }
        if (message.type === "presence") {
          setSession((current) =>
            current?.socket === socket && ["waiting", "active", "paused"].includes(current.participantState.state)
              ? { ...current, participantState: { ...current.participantState, presence: message.presence } as ParticipantState<TObservation, TAction, TCompletion> }
              : current
          );
          return;
        }
        if (message.type === "voice_status") {
          audioController.updateVoiceStatus(voiceStatusUpdate(message.voice));
          return;
        }
        if (message.type === "message") {
          setSession((current) =>
            current?.socket === socket
              ? { ...current, conversation: appendConversation(current.conversation, playerMessage(message.message)) }
              : current
          );
          return;
        }
        if (message.type === "action_rejected") {
          setError(`Action rejected: ${message.code}`);
          return;
        }
        if (message.type === "error") {
          setError(errorText(message.code));
          return;
        }
        const unreachable: never = message;
        throw new Error(`unhandled server message ${String(unreachable)}`);
      });
      socket.addEventListener("close", () => {
        setSession((current) => {
          const next = current?.socket === socket ? { ...current, synchronization: "reconnecting" as const } : current;
          sessionRef.current = next;
          return next;
        });
        const current = sessionRef.current;
        if (!reconnectEnabledRef.current || current?.socket !== socket || current.participantState.state === "ended") return;
        void apiClient.getParticipantState<TObservation, TAction, TCompletion>().then((reconciled) => {
          if (reconciled.state === "ended") {
            setSession((current) => current?.socket === socket
              ? { ...current, participantState: reconciled }
              : current);
            return;
          }
          scheduleGameReconnectRef.current(reconciled as ParticipantState<TObservation, TAction, TCompletion>);
        }).catch(() => scheduleGameReconnectRef.current(participantState));
      });
    },
    [apiClient, audioController]
  );
  connectSessionRef.current = connectSession;

  const ensureParticipant = useCallback(async () => {
    if (!publicConfig) throw new Error("Experiment config has not loaded.");
    if (!requiredConsentsAccepted(publicConfig, consentDecisions)) {
      throw new Error("Please accept all required consents before entering the waiting room.");
    }
    await apiClient.register();
    if (publicConfig.consents.length > 0) {
      await apiClient.acceptConsents(consentDecisions);
    }
  }, [apiClient, consentDecisions, publicConfig]);

  const createDirectRoom = useCallback(async () => {
    if (entering) return;
    setEntering(true);
    try {
      setError("");
      await ensureParticipant();
      reconnectEnabledRef.current = true;
      await connectSession(await apiClient.join<TObservation, TAction, TCompletion>());
    } catch (caught) {
      reconnectEnabledRef.current = false;
      setError(errorMessage(caught, "Could not create the waiting room."));
    } finally {
      setEntering(false);
    }
  }, [apiClient, connectSession, ensureParticipant, entering]);

  const prepareVoice = useCallback(async (deviceId = ""): Promise<boolean> => {
    if (!enabled) return false;
    const generation = ++voicePreparationGenerationRef.current;
    try {
      setError("");
      setSelectedAudioInputId(deviceId);
      await audioController.prepare(deviceId, selectedAudioInputLabel(audioInputs, deviceId));
      if (generation !== voicePreparationGenerationRef.current) return false;
      const refreshedInputs = await refreshAudioInputs();
      if (generation !== voicePreparationGenerationRef.current) return false;
      const activeDeviceLabel = audioController.snapshot().voicePreflight.deviceLabel;
      const activeDevice = refreshedInputs.find((device) => device.label === activeDeviceLabel);
      setSelectedAudioInputId(deviceId || activeDevice?.deviceId || "");
      return true;
    } catch (caught) {
      if (generation === voicePreparationGenerationRef.current) {
        setError(errorMessage(caught, "Microphone permission was not granted."));
      }
      return false;
    }
  }, [audioController, audioInputs, refreshAudioInputs, enabled]);

  /** Builds the current room-bound context shared by voice connection and mute operations. */
  const currentAudioContext = useCallback((): AudioSessionContext | null => {
    if (!session || session.participantState.state === "registered" || session.participantState.state === "ended") return null;
    const participantState = session.participantState;
    const selectedAudioInput = audioInputs.find((device) => device.deviceId === selectedAudioInputId);
    const logVoice = (event: string, metadata: Record<string, unknown> = {}) => {
      apiClient.postVoiceDiagnostic(participantState.public_session_id, event, metadata);
    };
    return {
      sessionId: participantState.public_session_id,
      role: participantState.role,
      selectedAudioInputId,
      selectedAudioInputLabel: selectedAudioInput?.label || null,
      getAudioSession: () => apiClient.getAudioSession(participantState.public_session_id),
      logVoice,
      onVoiceStatus: (status) => audioController.updateVoiceStatus(status)
    };
  }, [apiClient, audioController, audioInputs, selectedAudioInputId, session]);

  /** Connects the prepared microphone without changing the participant's desired mute state. */
  const connectVoice = useCallback(async () => {
    const context = currentAudioContext();
    if (!context) return;
    setError("");
    try {
      await audioController.connect(context);
    } catch (caught) {
      setError(errorMessage(caught, "Could not start voice chat."));
      setVoiceReconnectGeneration((generation) => generation + 1);
    }
  }, [audioController, currentAudioContext]);

  /** Applies a participant-requested mute state while retaining the live voice transport. */
  const setMicrophoneMuted = useCallback(async (muted: boolean) => {
    const context = currentAudioContext();
    if (!context) return;
    await audioController.setMicrophoneMuted(muted, context);
  }, [audioController, currentAudioContext]);

  useEffect(() => {
    let cancelled = false;
    apiClient
      .getExperiment()
      .then((config) => {
        if (!cancelled) setPublicConfig(config);
      })
      .catch((caught) => {
        if (!cancelled) setError(errorMessage(caught, "Could not load experiment config."));
      })
      .finally(() => {
        if (!cancelled) setConfigLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [apiClient]);

  // A reload reuses the tab-scoped credential and asks the server for the same room.
  useEffect(() => {
    if (!publicConfig || session || typeof apiClient.hasCredential !== "function" || !apiClient.hasCredential()) return;
    reconnectEnabledRef.current = true;
    void apiClient.join<TObservation, TAction, TCompletion>()
      .then(connectSession)
      .catch(() => {
        reconnectEnabledRef.current = false;
      });
  }, [apiClient, connectSession, publicConfig, session]);

  // Rechecks closed intake so a waiting visitor can proceed after an administrator opens it.
  useEffect(() => {
    if (!publicConfig || experimentAllowsIntake(publicConfig.status) || session) return;
    const timer = window.setInterval(() => {
      void apiClient
        .getExperiment()
        .then(setPublicConfig)
        .catch((caught) => setError(errorMessage(caught, "Could not refresh experiment status.")));
    }, 5_000);
    return () => window.clearInterval(timer);
  }, [apiClient, publicConfig?.status, session]);

  useEffect(() => {
    void refreshAudioInputs();
    if (!navigator.mediaDevices?.addEventListener) return;
    const onDeviceChange = () => void refreshAudioInputs();
    navigator.mediaDevices.addEventListener("devicechange", onDeviceChange);
    return () => navigator.mediaDevices.removeEventListener("devicechange", onDeviceChange);
  }, [refreshAudioInputs]);

  useEffect(() => audioController.subscribe((snapshot) => {
    setVoiceStatus(snapshot.voiceStatus);
    setVoicePreflight(snapshot.voicePreflight);
  }), [audioController]);

  useEffect(() => {
    if (status.connected) setVoiceReconnectGeneration(0);
  }, [status.connected]);

  useEffect(() => {
    sessionRef.current = session;
  }, [session]);

  // One-second transport heartbeat. It is deliberately not research activity and
  // therefore never extends the server's meaningful session-idle deadline.
  useEffect(() => {
    if (!session || session.participantState.state === "ended") return;
    const socket = session.socket;
    const timer = window.setInterval(() => {
      if (socket.readyState === WebSocket.OPEN) socket.send(JSON.stringify({ type: "heartbeat" }));
    }, CLIENT_HEARTBEAT_INTERVAL_MS);
    return () => window.clearInterval(timer);
  }, [session?.socket, session?.participantState.state]);

  useEffect(() => () => {
    endCurrentSession();
  }, [endCurrentSession]);

  useEffect(() => {
    const onBrowserTeardown = () => endCurrentSession();
    window.addEventListener("pagehide", onBrowserTeardown);
    window.addEventListener("beforeunload", onBrowserTeardown);
    return () => {
      window.removeEventListener("pagehide", onBrowserTeardown);
      window.removeEventListener("beforeunload", onBrowserTeardown);
    };
  }, [endCurrentSession]);

  useEffect(() => {
    if (session?.synchronization !== "connected" || !enabled || !voicePreflight.ready || status.connected || status.connecting) return;
    const delays = [1_000, 2_000, 5_000, 10_000];
    const timer = window.setTimeout(
      () => void connectVoice(),
      delays[Math.min(voiceReconnectGeneration, delays.length - 1)]
    );
    return () => window.clearTimeout(timer);
  }, [connectVoice, session?.synchronization, enabled, voicePreflight.ready, status.connected, status.connecting, voiceReconnectGeneration]);

  if (configLoading || !publicConfig) {
    return (
      <StartupShell
        institution={publicConfig?.institution}
        heading="Loading experiment"
        body="Connecting to the experiment server."
        error={error}
      />
    );
  }

  if (session?.participantState.state === "ended") {
    const result = session.participantState.result;
    return (
      <SessionOutcomePanel
        outcome={result.outcome}
        reason={result.reason}
        handoff={result.handoff}
        recruitment={publicConfig.recruitment}
        completionContent={result.outcome === "completed" && result.completion !== null
          ? renderCompletion?.(result.completion)
          : null}
      />
    );
  }

  if (session?.participantState.state === "active" || session?.participantState.state === "paused") {
    const participantState = session.participantState;
    const activeSession: GameSession<TObservation, TAction, TCompletion> = {
      sessionId: participantState.public_session_id,
      role: participantState.role,
      observation: participantState.observation,
      transition: session.transition,
      availableActions: participantState.available_actions,
      conversation: session.conversation,
      presence: normalizePresence(participantState.presence),
      voiceStatus: status,
      voicePreflight,
      voiceEnabled: enabled,
      connected: session.synchronization === "connected",
      interactionEnabled: participantState.state === "active" && session.synchronization === "connected",
      sendAction: (action) => sendActionIfGameActive(apiClient, session, action),
      sendMessage: (text) => sendMessageIfGameActive(apiClient, session, text),
      setMicrophoneMuted,
      leave
    };
    return (
      <>
        {renderGame(activeSession)}
        <IdleDeadlineNotice deadlineAt={participantState.idle_deadline_at} />
        {participantState.state === "paused" && participantState.reason.type === "partner_reconnecting" && <PartnerReconnectNotice deadlineAt={participantState.reason.deadline_at} />}
        {error && <p className="online-error">{error}</p>}
      </>
    );
  }

  if (session) {
    const presence = session.participantState.state === "waiting"
      ? normalizePresence(session.participantState.presence)
      : {};
    return (
      <StartupShell
        gameName={publicConfig.gameName}
        institution={publicConfig.institution}
        heading="Waiting for another participant"
        body="The game starts automatically when your partner is ready."
        error={error}
      >
        {session.participantState.state === "waiting" && (
          <WaitingRoomNotice
            deadlineAt={session.participantState.waiting_deadline_at}
            prolific={publicConfig.recruitment?.provider === "prolific"}
            startedAt={session.participantState.waiting_started_at}
          />
        )}
        <ReadinessBoard
          connected={session.synchronization === "connected"}
          enabled={enabled}
          presence={presence}
          status={status}
        />
        {enabled && <TranscriptionProgress connected={session.synchronization === "connected"} status={status} />}
        <div className="voice-preflight">
          <div>
            <strong>Voice chat</strong>
            <span>{enabled ? status.message : "Voice is disabled for this experiment"}</span>
          </div>
        </div>
        <div className="lobby-actions">
          <button onClick={leave}>Leave waiting room</button>
        </div>
      </StartupShell>
    );
  }

  if (!experimentAllowsIntake(publicConfig.status)) {
    return (
      <StartupShell
        gameName={publicConfig.gameName}
        institution={publicConfig.institution}
        heading="Experiment not accepting participants"
        body="The experiment is not accepting new participants. Please return after the experimenter opens intake."
        error={error}
      />
    );
  }

  return (
    <StartupShell
      gameName={publicConfig.gameName}
      institution={publicConfig.institution}
      heading="Join experiment"
      error={error}
    >
      {publicConfig.consents.length > 0 && (
        <div className="consent-list">
          {publicConfig.participantInformationUrl && (
            <p className="participant-information">
              <a href={publicConfig.participantInformationUrl} rel="noreferrer" target="_blank">
                Participant information
                {publicConfig.participantInformationVersion && ` (${publicConfig.participantInformationVersion})`}
              </a>
            </p>
          )}
          {publicConfig.consents.map((consent) => (
            <label className="consent-row" key={consent.id}>
              <input
                checked={Boolean(consentDecisions[consent.id])}
                onChange={(event) => setConsentDecisions((current) => ({ ...current, [consent.id]: event.target.checked }))}
                type="checkbox"
              />
              <span>
                <strong>
                  {consent.title}
                  {consent.required && " (required)"}
                </strong>
                <span>{consent.body}</span>
              </span>
            </label>
          ))}
        </div>
      )}
      {enabled && (
        <div className="voice-preflight">
          <div>
            <strong>Voice chat</strong>
            <span>{voicePreflight.ready ? "Microphone ready" : voicePreflight.message}</span>
          </div>
          <VoicePreparationControls
            audioInputs={audioInputs}
            enabled={enabled}
            onPrepareVoice={prepareVoice}
            onSelectedAudioInputChange={setSelectedAudioInputId}
            selectedAudioInputId={selectedAudioInputId}
            voicePreflight={voicePreflight}
          />
        </div>
      )}
      <div className="lobby-actions">
        <button disabled={!canEnter} onClick={createDirectRoom} type="button">
          Enter waiting room
        </button>
        {publicConfig.recruitment?.provider === "prolific" && publicConfig.recruitment.decline_url && (
          <a className="parlando-decline-consent" href={publicConfig.recruitment.decline_url}>
            Do not consent
          </a>
        )}
      </div>
    </StartupShell>
  );
}

/** Shared fixed-deadline guidance for an unmatched waiting session. */
export function WaitingRoomNotice({
  deadlineAt,
  prolific,
  startedAt
}: {
  deadlineAt: string;
  prolific: boolean;
  startedAt: string;
}) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 250);
    return () => window.clearInterval(timer);
  }, []);
  const deadline = Date.parse(deadlineAt);
  const started = Date.parse(startedAt);
  const remainingSeconds = Math.max(0, Math.ceil((deadline - now) / 1000));
  const maximumMinutes = Math.max(1, Math.ceil((deadline - started) / 60_000));
  const remainingMinutes = Math.floor(remainingSeconds / 60);
  const seconds = remainingSeconds % 60;
  const terminalGuidance = prolific
    ? "If you leave now or the countdown expires, Parlando will end this waiting session and show your Unmatched completion path for returning to Prolific."
    : "If you leave now or the countdown expires, Parlando will end this waiting session as unmatched.";
  return (
    <section aria-live="polite" className="parlando-waiting-room" role="status">
      <strong>Waiting for your partner</strong>
      <span className="parlando-waiting-countdown">
        {remainingMinutes}:{String(seconds).padStart(2, "0")} remaining
      </span>
      <span>The maximum wait is {maximumMinutes} minute{maximumMinutes === 1 ? "" : "s"}. Keep this tab open.</span>
      <span>{terminalGuidance}</span>
    </section>
  );
}

/** Stylable, self-updating notice shown while a required partner may reconnect. */
export function PartnerReconnectNotice({ deadlineAt }: { deadlineAt: string }) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 250);
    return () => window.clearInterval(timer);
  }, []);
  const seconds = Math.max(0, Math.ceil((Date.parse(deadlineAt) - now) / 1000));
  return (
    <section aria-live="polite" className="parlando-partner-reconnect" role="status">
      <strong>Your partner lost their connection</strong>
      <span>The game is paused while they reconnect. It will end in {seconds} second{seconds === 1 ? "" : "s"}.</span>
    </section>
  );
}

/** Warns both participants when the shared meaningful-activity deadline is close. */
export function IdleDeadlineNotice({ deadlineAt }: { deadlineAt: string }) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, []);
  const seconds = Math.max(0, Math.ceil((Date.parse(deadlineAt) - now) / 1_000));
  if (seconds > 60) return null;
  return (
    <section aria-live="polite" className="parlando-idle-warning" role="status">
      <strong>Inactivity limit approaching</strong>
      <span>Send a message or make a game action within {seconds} second{seconds === 1 ? "" : "s"} to keep the session active.</span>
    </section>
  );
}

/** Standard terminal surface for provider-neutral outcomes and recruitment handoff. */
function SessionOutcomePanel({
  outcome,
  reason,
  handoff,
  recruitment,
  completionContent
}: {
  outcome: ParticipantOutcome | null;
  reason: string | null;
  handoff: RecruitmentHandoff | null;
  recruitment?: ExperimentInfo["recruitment"];
  completionContent?: ReactNode;
}) {
  const heading = outcome === "completed" ? "Session complete" : "Session ended";
  return (
    <section className={`parlando-session-outcome outcome-${outcome ?? "unknown"}`}>
      {completionContent ? (
        <div className="parlando-game-completion">{completionContent}</div>
      ) : (
        <>
          <h1>{heading}</h1>
          <p>{outcomeText(outcome, reason)}</p>
        </>
      )}
      {handoff && (
        <ProlificHandoff handoff={handoff} />
      )}
    </section>
  );
}

/** Premade Prolific completion-code widget appended to a game's terminal content. */
export function ProlificHandoff({ handoff }: { handoff: RecruitmentHandoff }) {
  const [copyState, setCopyState] = useState<"idle" | "copied" | "failed">("idle");

  /** Copies the completion code and exposes the result to sighted and screen-reader users. */
  async function copyCode(): Promise<void> {
    try {
      if (!navigator.clipboard) throw new Error("Clipboard API unavailable");
      await navigator.clipboard.writeText(handoff.code);
      setCopyState("copied");
    } catch {
      setCopyState("failed");
    }
  }

  return (
    <div className="parlando-recruitment-handoff">
      <p>Use this completion code in Prolific:</p>
      <div className="parlando-completion-code-row">
        <strong className="parlando-completion-code">{handoff.code}</strong>
        <button
          aria-label="Copy Prolific completion code"
          className="parlando-copy-completion-code"
          onClick={copyCode}
          type="button"
        >
          {copyState === "copied" ? "Copied" : "Copy"}
        </button>
      </div>
      <span aria-live="polite" className="parlando-copy-status">
        {copyState === "copied" && "Copied to clipboard."}
        {copyState === "failed" && "Could not copy automatically. Select the code and copy it manually."}
      </span>
      <a href={handoff.url}>Return to Prolific</a>
    </div>
  );
}

/** Converts stable outcomes into concise participant-facing explanations. */
function outcomeText(outcome: ParticipantOutcome | null, reason: string | null): string {
  switch (outcome) {
    case "completed": return "Thank you. Your responses have been recorded.";
    case "left_waiting_room": return "You left before a partner became available. Use the Unmatched completion path below to return to Prolific.";
    case "left_game": return "You left after the game started. Your responses up to that point have been recorded.";
    case "participant_inactive": return "The game ended because a required response was not received from you.";
    case "connection_lost": return "The session ended because your connection did not return before the reconnect deadline.";
    case "partner_left": return "Your partner left or could not reconnect, so the session cannot continue.";
    case "partner_unavailable": return "No partner became available before the waiting period ended.";
    case "idle_limit_reached": return "The session ended because neither participant produced meaningful activity before the inactivity deadline.";
    case "technical_failure": return "A technical problem prevented the session from continuing.";
    case "lifetime_limit_reached": return "The session reached its absolute lifetime limit and could not continue.";
    default: return reason ? `The session ended (${reason}).` : "The session can no longer continue.";
  }
}

function StartupShell({
  body,
  children,
  error,
  gameName,
  heading,
  institution
}: {
  body?: string;
  children?: ReactNode;
  error: string;
  gameName?: string;
  heading?: string;
  institution?: string | null;
}) {
  return (
    <section className="lobby-panel">
      <div className="lobby-heading">
        <p className="platform-label">{platformLabel(institution)}</p>
        {gameName && <p className="game-name">{gameName}</p>}
        {heading && <h1>{heading}</h1>}
      </div>
      {body && (
        <div className="lobby-copy">
          <p>{body}</p>
        </div>
      )}
      {children}
      {error && <p className="online-error">{error}</p>}
    </section>
  );
}

function ReadinessBoard({
  connected,
  enabled,
  presence,
  status
}: {
  connected: boolean;
  enabled: boolean;
  presence: Presence;
  status: VoiceStatus;
}) {
  const aConnected = Boolean(presence.A?.connected);
  const bConnected = Boolean(presence.B?.connected);
  return (
    <div className="seat-grid">
      <div className={aConnected ? "seat-ready" : ""}>
        <strong>Player A</strong>
        <span>{aConnected ? "Connected" : "Waiting"}</span>
      </div>
      <div className={bConnected ? "seat-ready" : ""}>
        <strong>Player B</strong>
        <span>{bConnected ? "Connected" : "Waiting"}</span>
      </div>
      {enabled && (
        <div className={status.transcriptionReady ? "seat-ready" : ""}>
          <strong>Transcription Service</strong>
          <span>{status.transcriptionReady ? "Ready" : connected ? status.transcriptionMessage : "Not started"}</span>
        </div>
      )}
    </div>
  );
}

/** @internal Creates the browser audio controller used by ParticipantApp. */
export function createDefaultAudioController(): AudioSessionController {
  return new AudioSessionController({
    microphone: new MicrophoneSource(),
    sink: new ParlandoAudioSink()
  });
}

/** @internal
 * Renders the participant's microphone-preparation controls.
 *
 * The first action prepares the browser default. Once permission reveals named devices, the active
 * microphone becomes the selected dropdown value and choosing another value replaces the stream.
 */
export function VoicePreparationControls({
  audioInputs,
  enabled,
  onPrepareVoice,
  onSelectedAudioInputChange,
  selectedAudioInputId,
  voicePreflight = initialVoicePreflight
}: {
  audioInputs: MediaDeviceInfo[];
  enabled: boolean;
  onPrepareVoice: (deviceId?: string) => boolean | void | Promise<boolean | void>;
  onSelectedAudioInputChange: (value: string) => void;
  selectedAudioInputId: string;
  voicePreflight?: VoicePreflight;
}) {
  const availableMicrophones = selectableAudioInputs(audioInputs);
  return (
    <>
      {(voicePreflight.ready || voicePreflight.preparing) && (
        <select
          aria-label="Microphone input"
          disabled={voicePreflight.preparing || availableMicrophones.length < 2}
          onChange={(event) => {
            const deviceId = event.target.value;
            onSelectedAudioInputChange(deviceId);
            void Promise.resolve(onPrepareVoice(deviceId)).catch(() => undefined);
          }}
          value={selectedAudioInputId}
        >
          {!availableMicrophones.some((device) => device.deviceId === selectedAudioInputId) && (
            <option value="">{participantMicrophoneLabel(voicePreflight.deviceLabel)}</option>
          )}
          {availableMicrophones.map((device, index) => (
            <option key={device.deviceId || `audio-${index}`} value={device.deviceId}>
              {participantMicrophoneLabel(device.label || `Microphone ${index + 1}`)}
            </option>
          ))}
        </select>
      )}
      {voicePreflight.micProbeActive && <MicrophoneLevelMeter active={voicePreflight.micProbeActive} label="Level" level={voicePreflight.micLevel} />}
      {!voicePreflight.ready && !voicePreflight.preparing && (
        <button disabled={!enabled} onClick={() => void onPrepareVoice("")} type="button">
          Prepare voice
        </button>
      )}
    </>
  );
}

/** @internal Reports whether the standard application should prepare voice. */
export function isVoiceEnabled(config: ExperimentInfo | null): boolean {
  return Boolean(config?.voice?.enabled);
}

/** @internal Returns concrete microphone inputs, excluding the synthetic default-device alias. */
export function selectableAudioInputs(audioInputs: MediaDeviceInfo[]): MediaDeviceInfo[] {
  return audioInputs.filter((device) => device.deviceId !== "default");
}

/** @internal Removes browser-added USB identifiers from a microphone name. */
export function participantMicrophoneLabel(label: string): string {
  return label.replace(/\s*[([][0-9a-f]{4}:[0-9a-f]{4}[)\]]\s*$/i, "").trim() || "Microphone";
}

/** @internal Formats the stable platform label with an optional operating institution. */
export function platformLabel(institution?: string | null): string {
  const name = institution?.trim();
  return name ? `Parlando · ${name}` : "Parlando";
}

/** @internal Normalizes wire presence into the public two-role shape. */
export function normalizePresence(presence: Record<string, unknown> | undefined): Presence {
  return {
    A: normalizeSeat(presence?.A),
    B: normalizeSeat(presence?.B)
  };
}

function normalizeSeat(value: unknown): Presence["A"] {
  if (!value || typeof value !== "object") return undefined;
  const record = value as Record<string, unknown>;
  return {
    connected: Boolean(record.connected),
    audioReady: typeof record.audioReady === "boolean" ? record.audioReady : undefined
  };
}

/** @internal Normalizes a wire voice-status update. */
export function voiceStatusUpdate(voice: { audioReady?: boolean; transcriptionReady?: boolean; transcriptionStatus?: string } | undefined): Partial<VoiceStatus> {
  const update: Partial<VoiceStatus> = {};
  if (voice?.transcriptionStatus) update.transcriptionMessage = voice.transcriptionStatus;
  if (typeof voice?.transcriptionReady === "boolean") update.transcriptionReady = voice.transcriptionReady;
  return update;
}

function appendConversation(current: PlayerMessage[], message: PlayerMessage): PlayerMessage[] {
  if (current.some((candidate) => candidate.id === message.id)) return current;
  return [...current, message].slice(-50);
}

/** Maps stable protocol error codes onto the standard participant application's copy. */
function errorText(code: string): string {
  const messages: Record<string, string> = {
    action_too_large: "The action was too large.",
    internal_error: "The server could not complete the request.",
    invalid_action: "The action was not valid.",
    invalid_message: "The server could not read the game message.",
    message_rate_limited: "Please wait a moment before sending another message.",
    message_rejected: "The message could not be sent.",
    message_too_large: "The message was too long.",
    participant_left: "This session ended because a player left.",
    readiness_failed: "The session could not be started.",
    session_end_failed: "The session could not be ended cleanly.",
  };
  return messages[code] ?? "The server rejected the last request.";
}

// Closes the game WebSocket so the server records the same participant_disconnected event as the Leave action.
function closeSessionSocket(session: { socket: WebSocket } | null): void {
  if (session?.socket.readyState === WebSocket.OPEN || session?.socket.readyState === WebSocket.CONNECTING) {
    session.socket.close();
  }
}

function selectedAudioInputLabel(audioInputs: MediaDeviceInfo[], selectedAudioInputId: string): string {
  if (!selectedAudioInputId) return "Default microphone";
  return audioInputs.find((device) => device.deviceId === selectedAudioInputId)?.label || "Selected microphone";
}

function errorMessage(caught: unknown, fallback: string): string {
  return caught instanceof Error ? caught.message : fallback;
}
