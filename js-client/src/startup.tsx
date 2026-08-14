import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { AudioSessionController } from "./audio/audioSessionController.js";
import { ParlandoAudioSink } from "./audio/parlandoAudioSink.js";
import { MicrophoneSource } from "./audio/microphoneSource.js";
import { initialVoicePreflight, initialVoiceStatus, type VoicePreflight, type VoiceStatus } from "./audio/types.js";
import { requiredConsentsAccepted, transcriptionProgressForStatus, type PresenceState } from "./helpers.js";
import {
  ExperimentApiClient,
  type ConversationMessage,
  type PublicConfigResponse,
  type RoomResponse,
  type ServerMessage
} from "./protocol.js";

export interface ActiveParlandoSession<
  TState,
  TObservation,
  TAction,
  TEvent,
  TSummary = Record<string, unknown>
> {
  roomId: string;
  participantSessionId: string;
  role: string;
  state: TState | null;
  observation: TObservation | null;
  availableActions: TAction[];
  events: TEvent[];
  conversation: ConversationMessage[];
  presence: PresenceState;
  voiceStatus: VoiceStatus;
  voicePreflight: VoicePreflight;
  publicConfig: PublicConfigResponse;
  connected: boolean;
  completed: boolean;
  completionSummary: TSummary | null;
  sendAction(action: TAction): void;
  sendChatMessage(text: string): void;
  toggleVoice(): Promise<void>;
  leave(): void;
}

export interface ParlandoStartupGateProps<
  TState,
  TObservation,
  TAction,
  TEvent,
  TSummary = Record<string, unknown>
> {
  renderGame(session: ActiveParlandoSession<TState, TObservation, TAction, TEvent, TSummary>): ReactNode;
  apiClient?: ExperimentApiClient;
  createAudioController?: () => AudioSessionController;
}

interface RoomSession<TState, TObservation, TAction, TEvent, TSummary = Record<string, unknown>> {
  roomId: string;
  participantSessionId: string;
  role: string;
  state: TState | null;
  observation: TObservation | null;
  availableActions: TAction[];
  events: TEvent[];
  socket: WebSocket;
  connected: boolean;
  active: boolean;
  completed: boolean;
  completionSummary: TSummary | null;
  presence: PresenceState;
  conversation: ConversationMessage[];
}

export interface GameInputSession {
  socket: WebSocket;
  completed: boolean;
}

/** Returns the state update applied when the server announces game completion. */
export function completedSessionPatch<TSummary>(summary: TSummary | undefined): {
  completed: true;
  completionSummary: TSummary | null;
} {
  return { completed: true, completionSummary: summary ?? null };
}

/** Returns whether participant game-channel messages should still be sent. */
export function canSendGameMessage(session: { completed: boolean } | null): boolean {
  return Boolean(session && !session.completed);
}

/** Sends an action only while the reusable session is still accepting game input. */
export function sendActionIfGameActive<TAction>(
  apiClient: Pick<ExperimentApiClient, "sendAction">,
  session: GameInputSession | null,
  action: TAction
): void {
  if (!session || session.completed) return;
  apiClient.sendAction(session.socket, action);
}

/** Sends a chat message only while the reusable session is still accepting game input. */
export function sendChatMessageIfGameActive(
  apiClient: Pick<ExperimentApiClient, "sendChatMessage">,
  session: GameInputSession | null,
  text: string
): void {
  if (!session || session.completed) return;
  apiClient.sendChatMessage(session.socket, text);
}

export function ParlandoStartupGate<
  TState,
  TObservation = TState,
  TAction = unknown,
  TEvent = unknown,
  TSummary = Record<string, unknown>
>({
  renderGame,
  apiClient: providedApiClient,
  createAudioController = createDefaultAudioController
}: ParlandoStartupGateProps<TState, TObservation, TAction, TEvent, TSummary>) {
  const apiClient = useMemo(() => providedApiClient ?? new ExperimentApiClient(), [providedApiClient]);
  const audioControllerRef = useRef<AudioSessionController | null>(null);
  if (!audioControllerRef.current) audioControllerRef.current = createAudioController();
  const audioController = audioControllerRef.current;

  const [publicConfig, setPublicConfig] = useState<PublicConfigResponse | null>(null);
  const [configLoading, setConfigLoading] = useState(true);
  const [consentDecisions, setConsentDecisions] = useState<Record<string, boolean>>({});
  const [session, setSession] = useState<RoomSession<TState, TObservation, TAction, TEvent, TSummary> | null>(null);
  const [error, setError] = useState("");
  const [audioInputs, setAudioInputs] = useState<MediaDeviceInfo[]>([]);
  const [selectedAudioInputId, setSelectedAudioInputId] = useState("");
  const [voiceStatus, setVoiceStatus] = useState<VoiceStatus>(initialVoiceStatus);
  const [voicePreflight, setVoicePreflight] = useState<VoicePreflight>(initialVoicePreflight);
  const sessionRef = useRef<RoomSession<TState, TObservation, TAction, TEvent, TSummary> | null>(null);
  const automaticVoiceAttemptRef = useRef("");
  const consentReady = requiredConsentsAccepted(publicConfig, consentDecisions);
  const voiceEnabled = isVoiceEnabled(publicConfig);
  const canEnter = Boolean(publicConfig && consentReady && (!voiceEnabled || voicePreflight.ready));
  const startupTitle = resolveStartupTitle(publicConfig);

  const refreshAudioInputs = useCallback(async () => {
    if (!navigator.mediaDevices?.enumerateDevices) return [];
    const devices = await navigator.mediaDevices.enumerateDevices();
    const inputs = devices.filter((device) => device.kind === "audioinput");
    setAudioInputs(inputs);
    return inputs;
  }, []);

  const endCurrentSession = useCallback(() => {
    void audioController.disconnect(true);
    closeSessionSocket(sessionRef.current);
  }, [audioController]);

  const leave = useCallback(() => {
    endCurrentSession();
    setSession((current) => {
      closeSessionSocket(current);
      return null;
    });
    setError("");
  }, [endCurrentSession]);

  const connectRoom = useCallback(
    async (room: RoomResponse<TState, TObservation, TAction, TEvent>) => {
      const gameSession = await apiClient.getGameSession(room.room_id, room.participant_session_id);
      const socket = new WebSocket(apiClient.socketUrl(gameSession));
      setSession({
        roomId: room.room_id,
        participantSessionId: room.participant_session_id,
        role: room.role,
        state: room.state ?? null,
        observation: room.observation ?? null,
        availableActions: room.available_actions ?? [],
        events: room.events ?? [],
        socket,
        connected: false,
        active: false,
        completed: false,
        completionSummary: null,
        presence: normalizePresence(room.presence),
        conversation: room.conversation ?? []
      });

      socket.addEventListener("open", () => {
        socket.send(JSON.stringify({ type: "ready" }));
        setSession((current) => (current?.socket === socket ? { ...current, connected: true } : current));
      });
      socket.addEventListener("message", (event) => {
        const message = JSON.parse(event.data) as ServerMessage<TState, TObservation, TAction, TEvent, TSummary>;
        if (message.type === "roleAssigned") {
          setSession((current) =>
            current?.socket === socket
              ? {
                  ...current,
                  roomId: message.room_id,
                  participantSessionId: message.participant_session_id,
                  role: message.role,
                  state: message.state ?? null,
                  observation: message.observation ?? null,
                  availableActions: message.available_actions ?? [],
                  events: message.events ?? [],
                  conversation: message.conversation ?? current.conversation,
                  connected: true,
                  active: true
                }
              : current
          );
          return;
        }
        if (message.type === "stateChanged") {
          setSession((current) =>
            current?.socket === socket
              ? {
                  ...current,
                  state: message.state ?? null,
                  observation: message.observation ?? current.observation,
                  availableActions: message.available_actions ?? [],
                  events: [...(message.events ?? []), ...current.events].slice(0, 20),
                  conversation: message.conversation ?? current.conversation
                }
              : current
          );
          return;
        }
        if (message.type === "presenceChanged") {
          setSession((current) =>
            current?.socket === socket ? { ...current, presence: normalizePresence(message.presence) } : current
          );
          return;
        }
        if (message.type === "voiceStatusChanged") {
          audioController.updateVoiceStatus(voiceStatusUpdate(message.voice));
          return;
        }
        if (message.type === "conversationMessageAdded") {
          setSession((current) =>
            current?.socket === socket
              ? { ...current, conversation: appendConversation(current.conversation, message.conversation_message) }
              : current
          );
          return;
        }
        if (message.type === "completed") {
          setSession((current) =>
            current?.socket === socket ? { ...current, ...completedSessionPatch(message.summary) } : current
          );
          return;
        }
        if (message.type === "error") {
          setError(message.message ?? "Server rejected the last request.");
        }
      });
      socket.addEventListener("close", () => {
        setSession((current) => (current?.socket === socket ? { ...current, connected: false } : current));
      });
    },
    [apiClient, audioController]
  );

  const ensureParticipant = useCallback(async () => {
    if (!publicConfig) throw new Error("Experiment config has not loaded.");
    if (!requiredConsentsAccepted(publicConfig, consentDecisions)) {
      throw new Error("Please accept all required consents before entering the waiting room.");
    }
    const participant = await apiClient.createParticipant();
    if (publicConfig.consents.length > 0) {
      await apiClient.submitConsent(participant.participant_session_id, consentDecisions);
    }
    return participant.participant_session_id;
  }, [apiClient, consentDecisions, publicConfig]);

  const createDirectRoom = useCallback(async () => {
    try {
      setError("");
      const participantSessionId = await ensureParticipant();
      await connectRoom(await apiClient.createRoom<TState, TObservation, TAction, TEvent>(participantSessionId, "direct"));
    } catch (caught) {
      setError(errorMessage(caught, "Could not create the waiting room."));
    }
  }, [apiClient, connectRoom, ensureParticipant]);

  const prepareVoice = useCallback(async (deviceId = ""): Promise<boolean> => {
    if (!voiceEnabled) return false;
    try {
      setError("");
      setSelectedAudioInputId(deviceId);
      await audioController.prepare(deviceId, selectedAudioInputLabel(audioInputs, deviceId));
      const refreshedInputs = await refreshAudioInputs();
      const activeDeviceLabel = audioController.snapshot().voicePreflight.deviceLabel;
      const activeDevice = refreshedInputs.find((device) => device.label === activeDeviceLabel);
      setSelectedAudioInputId(deviceId || activeDevice?.deviceId || "");
      return true;
    } catch (caught) {
      setError(errorMessage(caught, "Microphone permission was not granted."));
      return false;
    }
  }, [audioController, audioInputs, refreshAudioInputs, voiceEnabled]);

  const toggleVoice = useCallback(async () => {
    if (!session) return;
    const selectedAudioInput = audioInputs.find((device) => device.deviceId === selectedAudioInputId);
    const logVoice = (event: string, metadata: Record<string, unknown> = {}) => {
      apiClient.postVoiceDiagnostic(session.roomId, session.participantSessionId, event, metadata);
    };
    await audioController.toggle(
      {
        roomId: session.roomId,
        participantSessionId: session.participantSessionId,
        role: session.role,
        selectedAudioInputId,
        selectedAudioInputLabel: selectedAudioInput?.label || null,
        getAudioSession: () => apiClient.getAudioSession(session.roomId, session.participantSessionId),
        logVoice,
        onVoiceStatus: (status) => audioController.updateVoiceStatus(status)
      }
    );
  }, [apiClient, audioController, audioInputs, selectedAudioInputId, session]);

  /** Connects the prepared microphone to the room transport exactly once automatically. */
  const connectVoice = useCallback(async () => {
    setError("");
    try {
      await toggleVoice();
    } catch (caught) {
      setError(errorMessage(caught, "Could not start voice chat."));
    }
  }, [toggleVoice]);

  useEffect(() => {
    let cancelled = false;
    apiClient
      .getPublicConfig()
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
    sessionRef.current = session;
  }, [session]);

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
    if (!session || !voiceEnabled || !voicePreflight.ready || voiceStatus.connected || voiceStatus.connecting) return;
    const attemptKey = `${session.roomId}:${session.participantSessionId}`;
    if (automaticVoiceAttemptRef.current === attemptKey) return;
    automaticVoiceAttemptRef.current = attemptKey;
    void connectVoice();
  }, [connectVoice, session, voiceEnabled, voicePreflight.ready, voiceStatus.connected, voiceStatus.connecting]);

  if (configLoading || !publicConfig) {
    return (
      <StartupShell
        title={startupTitle}
        institution={publicConfig?.institution}
        heading="Loading study"
        body="Connecting to the experiment server."
        error={error}
      />
    );
  }

  if (session?.active) {
    const activeSession: ActiveParlandoSession<TState, TObservation, TAction, TEvent, TSummary> = {
      roomId: session.roomId,
      participantSessionId: session.participantSessionId,
      role: session.role,
      state: session.state,
      observation: session.observation,
      availableActions: session.availableActions,
      events: session.events,
      conversation: session.conversation,
      presence: session.presence,
      voiceStatus,
      voicePreflight,
      publicConfig,
      connected: session.connected,
      completed: session.completed,
      completionSummary: session.completionSummary,
      sendAction: (action) => sendActionIfGameActive(apiClient, session, action),
      sendChatMessage: (text) => sendChatMessageIfGameActive(apiClient, session, text),
      toggleVoice,
      leave
    };
    return <>{renderGame(activeSession)}</>;
  }

  if (session) {
    return (
      <StartupShell
        title={startupTitle}
        institution={publicConfig.institution}
        heading="Starting game"
        body="The game starts when all required participants and services are ready."
        error={error}
      >
        <ReadinessBoard voiceEnabled={voiceEnabled} presence={session.presence} voiceStatus={voiceStatus} />
        {voiceEnabled && <StartupTranscriptionProgress voiceStatus={voiceStatus} />}
        <div className="voice-preflight">
          <div>
            <strong>Voice chat</strong>
            <span>{voiceEnabled ? voiceStatus.message : "Voice is disabled for this study"}</span>
          </div>
        </div>
        <div className="lobby-actions">
          <button onClick={leave}>Leave waiting room</button>
        </div>
      </StartupShell>
    );
  }

  return (
    <StartupShell
      title={startupTitle}
      institution={publicConfig.institution}
      error={error}
    >
      {publicConfig.consents.length > 0 && (
        <div className="consent-list">
          {publicConfig.participant_information_url && (
            <p className="participant-information">
              <a href={publicConfig.participant_information_url} rel="noreferrer" target="_blank">
                Participant information
                {publicConfig.participant_information_version && ` (${publicConfig.participant_information_version})`}
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
                <span dangerouslySetInnerHTML={{ __html: consent.body_html }} />
              </span>
            </label>
          ))}
        </div>
      )}
      {voiceEnabled && (
        <div className="voice-preflight">
          <div>
            <strong>Voice chat</strong>
            <span>{voicePreflight.ready ? "Microphone ready" : voicePreflight.message}</span>
          </div>
          <VoicePreparationControls
            audioInputs={audioInputs}
            voiceEnabled={voiceEnabled}
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
  heading,
  institution,
  title
}: {
  body?: string;
  children?: ReactNode;
  error: string;
  heading?: string;
  institution?: string | null;
  title: string;
}) {
  return (
    <section className="lobby-panel">
      <div className="lobby-heading">
        <p className="platform-label">{platformLabel(institution)}</p>
        <h1>{title}</h1>
      </div>
      {(heading || body) && (
        <div className="lobby-copy">
          {heading && <h2>{heading}</h2>}
          {body && <p>{body}</p>}
        </div>
      )}
      {children}
      {error && <p className="online-error">{error}</p>}
    </section>
  );
}

function ReadinessBoard({
  voiceEnabled,
  presence,
  voiceStatus
}: {
  voiceEnabled: boolean;
  presence: PresenceState;
  voiceStatus: VoiceStatus;
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
        <strong>Player B / Agent</strong>
        <span>{bConnected ? "Connected" : "Waiting"}</span>
      </div>
      {voiceEnabled && (
        <div className={voiceStatus.transcriptionReady ? "seat-ready" : ""}>
          <strong>Transcription Service</strong>
          <span>{voiceStatus.transcriptionReady ? "Ready" : voiceStatus.transcriptionMessage}</span>
        </div>
      )}
    </div>
  );
}

function StartupTranscriptionProgress({ voiceStatus }: { voiceStatus: VoiceStatus }) {
  const progress = transcriptionProgressForStatus(voiceStatus.transcriptionMessage, voiceStatus.transcriptionReady);
  return (
    <div className="transcription-progress" aria-label="Transcription service progress">
      <div>
        <strong>{voiceStatus.transcriptionReady ? "Transcription ready" : "Waiting for transcription service"}</strong>
        <span>{voiceStatus.transcriptionMessage}</span>
      </div>
      <div className="transcription-progress-track" aria-hidden="true">
        <span style={{ transform: `scaleX(${progress.value})` }} />
      </div>
      <ol>
        {progress.steps.map((step) => (
          <li className={step.done ? "done" : ""} key={step.label}>
            {step.label}
          </li>
        ))}
      </ol>
    </div>
  );
}

export function createDefaultAudioController(): AudioSessionController {
  return new AudioSessionController({
    microphone: new MicrophoneSource(),
    sink: new ParlandoAudioSink()
  });
}

/**
 * Renders the participant's microphone-preparation controls.
 *
 * The first action prepares the browser default. Once permission reveals named devices, the active
 * microphone becomes the selected dropdown value and choosing another value replaces the stream.
 */
export function VoicePreparationControls({
  audioInputs,
  voiceEnabled,
  onPrepareVoice,
  onSelectedAudioInputChange,
  selectedAudioInputId,
  voicePreflight = initialVoicePreflight
}: {
  audioInputs: MediaDeviceInfo[];
  voiceEnabled: boolean;
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
      {voicePreflight.micProbeActive && <MicLevelMeter active={voicePreflight.micProbeActive} label="Level" level={voicePreflight.micLevel} />}
      {!voicePreflight.ready && !voicePreflight.preparing && (
        <button disabled={!voiceEnabled} onClick={() => void onPrepareVoice("")} type="button">
          Prepare voice
        </button>
      )}
    </>
  );
}

function MicLevelMeter({ active, label, level }: { active: boolean; label: string; level: number }) {
  return (
    <div className="mic-meter">
      <span>{label}</span>
      <div className="mic-meter-track" aria-hidden="true">
        <span style={{ transform: `scaleX(${active ? level : 0})` }} />
      </div>
    </div>
  );
}

export function isVoiceEnabled(config: PublicConfigResponse | null): boolean {
  return Boolean(config?.voice?.enabled);
}

/** Returns concrete microphone inputs, excluding the browser's synthetic default-device alias. */
export function selectableAudioInputs(audioInputs: MediaDeviceInfo[]): MediaDeviceInfo[] {
  return audioInputs.filter((device) => device.deviceId !== "default");
}

/** Removes browser-added USB vendor/product identifiers from a participant-facing microphone name. */
export function participantMicrophoneLabel(label: string): string {
  return label.replace(/\s*[([][0-9a-f]{4}:[0-9a-f]{4}[)\]]\s*$/i, "").trim() || "Microphone";
}

/** Formats the stable platform label with an optional operating institution. */
export function platformLabel(institution?: string | null): string {
  const name = institution?.trim();
  return name ? `Parlando · ${name}` : "Parlando";
}

/** Resolves the participant-facing game title from the server's public study configuration. */
export function resolveStartupTitle(config: Pick<PublicConfigResponse, "study_name"> | null): string {
  return config?.study_name ?? "Parlando Experiment";
}

export function normalizePresence(presence: Record<string, unknown> | undefined): PresenceState {
  return {
    A: normalizeSeat(presence?.A),
    B: normalizeSeat(presence?.B)
  };
}

function normalizeSeat(value: unknown): PresenceState["A"] {
  if (!value || typeof value !== "object") return undefined;
  const record = value as Record<string, unknown>;
  return {
    participantSessionId:
      typeof record.participantSessionId === "string" ? record.participantSessionId : undefined,
    connected: Boolean(record.connected),
    audioReady: typeof record.audioReady === "boolean" ? record.audioReady : undefined
  };
}

export function voiceStatusUpdate(voice: { audioReady?: boolean; transcriptionReady?: boolean; transcriptionStatus?: string } | undefined): Partial<VoiceStatus> {
  const update: Partial<VoiceStatus> = {};
  if (voice?.transcriptionStatus) update.transcriptionMessage = voice.transcriptionStatus;
  if (typeof voice?.transcriptionReady === "boolean") update.transcriptionReady = voice.transcriptionReady;
  return update;
}

function appendConversation(current: ConversationMessage[], message: ConversationMessage): ConversationMessage[] {
  if (current.some((candidate) => candidate.id === message.id)) return current;
  return [...current, message].slice(-50);
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
