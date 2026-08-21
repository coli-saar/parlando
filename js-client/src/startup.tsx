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
  type ExperimentInfo,
  type JoinedSession,
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
  completed: boolean;
  completion: TCompletion | null;
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
  baseUrl?: string;
}

interface ParticipantAppRuntimeProps<TObservation, TAction, TCompletion = Record<string, unknown>> {
  renderGame(session: GameSession<TObservation, TAction, TCompletion>): ReactNode;
  apiClient: ParticipantClient;
  createAudioController: () => AudioSessionController;
}

interface LiveSession<TObservation, TAction, TCompletion = Record<string, unknown>> {
  sessionId: string;
  role: PlayerRole;
  observation: TObservation | null;
  transition: GameTransition<TAction> | null;
  availableActions: TAction[] | null;
  socket: WebSocket;
  connected: boolean;
  active: boolean;
  completed: boolean;
  completion: TCompletion | null;
  presence: Presence;
  conversation: PlayerMessage[];
}

/** @internal Minimal channel state used by source-level tests. */
export interface GameInputSession {
  socket: WebSocket;
  completed: boolean;
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
export function canSendGameMessage(session: { completed: boolean } | null): boolean {
  return Boolean(session && !session.completed);
}

/** @internal Sends an action only while the reusable session is still accepting game input. */
export function sendActionIfGameActive<TAction>(
  apiClient: Pick<ParticipantClient, "sendAction">,
  session: GameInputSession | null,
  action: TAction
): void {
  if (!session || session.completed) return;
  apiClient.sendAction(session.socket, action);
}

/** @internal Sends a chat message only while the reusable session is still accepting game input. */
export function sendMessageIfGameActive(
  apiClient: Pick<ParticipantClient, "sendMessage">,
  session: GameInputSession | null,
  text: string
): void {
  if (!session || session.completed) return;
  apiClient.sendMessage(session.socket, text);
}

/** Runs the standard participant lifecycle before rendering one game session. */
export function ParticipantApp<
  TObservation,
  TAction = unknown,
  TCompletion = Record<string, unknown>
>({ renderGame, baseUrl }: ParticipantAppProps<TObservation, TAction, TCompletion>) {
  const apiClient = useMemo(() => new ParticipantClient({ baseUrl }), [baseUrl]);
  return (
    <ParticipantAppRuntime
      apiClient={apiClient}
      createAudioController={createDefaultAudioController}
      renderGame={renderGame}
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
>({ renderGame, apiClient, createAudioController }: ParticipantAppRuntimeProps<TObservation, TAction, TCompletion>) {
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
  const connectSessionRef = useRef<(room: JoinedSession<TObservation, TAction>) => Promise<void>>(async () => {});
  const scheduleGameReconnectRef = useRef<(room: JoinedSession<TObservation, TAction>) => void>(() => {});
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

  const leave = useCallback(() => {
    apiClient.leaveSession(sessionRef.current?.socket ?? null);
    endCurrentSession();
    setSession((current) => {
      closeSessionSocket(current);
      return null;
    });
    setError("");
  }, [apiClient, endCurrentSession]);

  const scheduleGameReconnect = useCallback(
    (room: JoinedSession<TObservation, TAction>) => {
      const current = sessionRef.current;
      if (!reconnectEnabledRef.current || !current || current.completed || reconnectTimerRef.current !== null) return;
      if (reconnectStartedAtRef.current === 0) reconnectStartedAtRef.current = Date.now();
      if (Date.now() - reconnectStartedAtRef.current >= 5 * 60_000) {
        setError("The five-minute reconnection window expired. Please leave and start a new session.");
        return;
      }
      const delays = [1_000, 2_000, 5_000, 10_000];
      const remaining = 5 * 60_000 - (Date.now() - reconnectStartedAtRef.current);
      const delay = Math.min(delays[Math.min(reconnectAttemptsRef.current, delays.length - 1)], remaining);
      reconnectAttemptsRef.current += 1;
      reconnectTimerRef.current = window.setTimeout(() => {
        reconnectTimerRef.current = null;
        void connectSessionRef.current(room).catch((caught) => {
          setError(errorMessage(caught, "Could not reconnect to the game channel."));
          scheduleGameReconnectRef.current(room);
        });
      }, delay);
    },
    []
  );
  scheduleGameReconnectRef.current = scheduleGameReconnect;

  const connectSession = useCallback(
    async (room: JoinedSession<TObservation, TAction>) => {
      const gameSession = await apiClient.getGameSession(room.sessionId);
      const socket = new WebSocket(apiClient.socketUrl(gameSession));
      setSession((current) => {
        const next = current?.sessionId === room.sessionId
          ? { ...current, socket, connected: false }
          : {
            sessionId: room.sessionId,
            role: room.role,
            observation: room.observation,
            transition: null,
            availableActions: room.availableActions,
            socket,
            connected: false,
            active: false,
            completed: false,
            completion: null,
            presence: room.presence,
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
        setSession((current) => (current?.socket === socket ? { ...current, connected: true } : current));
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
          message = JSON.parse(event.data) as ServerMessage<TObservation, TAction, TCompletion>;
          if (message.protocol_version !== 1) throw new Error("unsupported protocol version");
        } catch {
          setError("The server sent an invalid game message.");
          socket.close(1002, "Invalid server message");
          return;
        }
        if (message.type === "session_started") {
          setSession((current) =>
            current?.socket === socket
              ? {
                  ...current,
                  sessionId: message.public_session_id,
                  role: message.role,
                  observation: message.observation,
                  transition: null,
                  availableActions: message.available_actions,
                  connected: true,
                  active: true
                }
              : current
          );
          return;
        }
        if (message.type === "transition") {
          setSession((current) =>
            current?.socket === socket
              ? {
                  ...current,
                  transition: { actor: message.actor, action: message.action },
                  observation: message.observation,
                  availableActions: message.available_actions
                }
              : current
          );
          return;
        }
        if (message.type === "presence") {
          setSession((current) =>
            current?.socket === socket ? { ...current, presence: normalizePresence(message.presence) } : current
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
        if (message.type === "completed") {
          setSession((current) =>
            current?.socket === socket ? { ...current, ...completedSessionPatch(message.completion) } : current
          );
          return;
        }
        if (message.type === "action_rejected") {
          setError(`Action rejected: ${message.code}`);
          return;
        }
        if (message.type === "abandoned") {
          reconnectEnabledRef.current = false;
          void audioController.disconnect(true);
          socket.close();
          setSession(null);
          setError(errorText(message.code));
          return;
        }
        if (message.type === "error") {
          setError(errorText(message.code));
        }
      });
      socket.addEventListener("close", () => {
        setSession((current) => {
          const next = current?.socket === socket ? { ...current, connected: false } : current;
          sessionRef.current = next;
          return next;
        });
        const current = sessionRef.current;
        if (!reconnectEnabledRef.current || current?.socket !== socket || current.completed) return;
        scheduleGameReconnectRef.current(room);
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
      await connectSession(await apiClient.join<TObservation, TAction>());
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
    if (!session) return null;
    const selectedAudioInput = audioInputs.find((device) => device.deviceId === selectedAudioInputId);
    const logVoice = (event: string, metadata: Record<string, unknown> = {}) => {
      apiClient.postVoiceDiagnostic(session.sessionId, event, metadata);
    };
    return {
      sessionId: session.sessionId,
      role: session.role,
      selectedAudioInputId,
      selectedAudioInputLabel: selectedAudioInput?.label || null,
      getAudioSession: () => apiClient.getAudioSession(session.sessionId),
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
    if (!session) return;
    const socket = session.socket;
    const timer = window.setInterval(() => {
      if (socket.readyState === WebSocket.OPEN) socket.send(JSON.stringify({ type: "heartbeat" }));
    }, CLIENT_HEARTBEAT_INTERVAL_MS);
    return () => window.clearInterval(timer);
  }, [session?.socket]);

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
    if (!session?.connected || !enabled || !voicePreflight.ready || status.connected || status.connecting) return;
    const delays = [1_000, 2_000, 5_000, 10_000];
    const timer = window.setTimeout(
      () => void connectVoice(),
      delays[Math.min(voiceReconnectGeneration, delays.length - 1)]
    );
    return () => window.clearTimeout(timer);
  }, [connectVoice, session?.connected, enabled, voicePreflight.ready, status.connected, status.connecting, voiceReconnectGeneration]);

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

  if (session?.active) {
    const activeSession: GameSession<TObservation, TAction, TCompletion> = {
      sessionId: session.sessionId,
      role: session.role,
      observation: session.observation as TObservation,
      transition: session.transition,
      availableActions: session.availableActions,
      conversation: session.conversation,
      presence: session.presence,
      voiceStatus: status,
      voicePreflight,
      voiceEnabled: enabled,
      connected: session.connected,
      completed: session.completed,
      completion: session.completion,
      sendAction: (action) => sendActionIfGameActive(apiClient, session, action),
      sendMessage: (text) => sendMessageIfGameActive(apiClient, session, text),
      setMicrophoneMuted,
      leave
    };
    return (
      <>
        {renderGame(activeSession)}
        {error && <p className="online-error">{error}</p>}
      </>
    );
  }

  if (session) {
    return (
      <StartupShell
        gameName={publicConfig.gameName}
        institution={publicConfig.institution}
        heading="Starting session"
        body="The game starts when all required participants and services are ready."
        error={error}
      >
        <ReadinessBoard
          connected={session.connected}
          enabled={enabled}
          presence={session.presence}
          status={status}
        />
        {enabled && <TranscriptionProgress connected={session.connected} status={status} />}
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
        <button disabled={!canEnter} onClick={createDirectRoom}>
          Enter waiting room
        </button>
      </div>
    </StartupShell>
  );
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
