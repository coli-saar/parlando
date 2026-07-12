import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  AudioSessionController,
  ExperimentApiClient,
  MicrophoneSource,
  initialVoicePreflight,
  initialVoiceStatus,
  type ConversationMessage,
  type MatchmakingResponse,
  type PublicConfigResponse,
  type RoomResponse,
  type ServerMessage,
  type VoicePreflight,
  type VoiceStatus
} from "@parlando/client";
import { LiveKitPartnerAudioSink } from "@parlando/client/livekit";
import { SpeechmaticsTranscriptionSink } from "@parlando/client/speechmatics";
import {
  MicLevelMeter,
  TranscriptionProgress,
  TranscriptionStatusChip,
  VoiceJoinButton,
  VoicePreparationControls,
  VoiceStatusChip
} from "@parlando/client/react";
import {
  cells,
  devices,
  doors,
  mapHeight,
  mapWidth,
  roomAtPosition,
  roomById,
  roomRegions
} from "./game/level";
import {
  availableActions,
  deriveSystems,
  describeAction,
  initialState
} from "./game/stateEngine";
import type { DeviceDefinition, Direction, GameAction, PlayerId, Position, StationState } from "./game/types";
import type { ObservationEvent, StationObservation } from "./game/types";
const movementKeys: Record<string, Direction> = {
  arrowup: "up",
  arrowdown: "down",
  arrowleft: "left",
  arrowright: "right"
};

type GameRoomResponse = RoomResponse<StationState, StationObservation, GameAction, ObservationEvent>;
type GameMatchmakingResponse = MatchmakingResponse<StationState, StationObservation, GameAction, ObservationEvent>;
type GameServerMessage = ServerMessage<StationState, StationObservation, GameAction, ObservationEvent>;

export function App() {
  const [session, setSession] = useState<OnlineSession | null>(null);
  const [waitingParticipantId, setWaitingParticipantId] = useState<string | null>(null);
  const [displayName, setDisplayName] = useState("");
  const [publicConfig, setPublicConfig] = useState<PublicConfigResponse | null>(null);
  const [consentDecisions, setConsentDecisions] = useState<Record<string, boolean>>({});
  const [onlineError, setOnlineError] = useState("");
  const [preview, setPreview] = useState<ActionPreview | null>(null);
  const [chatDraft, setChatDraft] = useState("");
  const [audioInputs, setAudioInputs] = useState<MediaDeviceInfo[]>([]);
  const [selectedAudioInputId, setSelectedAudioInputId] = useState("");
  const [voiceStatus, setVoiceStatus] = useState<VoiceStatus>(initialVoiceStatus);
  const [voicePreflight, setVoicePreflight] = useState<VoicePreflight>(initialVoicePreflight);
  const audioControllerRef = useRef<AudioSessionController | null>(null);
  if (!audioControllerRef.current) {
    audioControllerRef.current = new AudioSessionController({
      microphone: new MicrophoneSource(),
      sinks: [new LiveKitPartnerAudioSink(), new SpeechmaticsTranscriptionSink()]
    });
  }
  const audioController = audioControllerRef.current;
  const apiClient = useMemo(() => new ExperimentApiClient(), []);
  const state = session?.observation ?? session?.state ?? initialState;
  const systems = useMemo(() => deriveSystems(state), [state]);
  const serverAvailableActions = session?.availableActions ?? [];
  const eventLog = session?.eventLog ?? [];
  const assignedRole = session?.role === "A" || session?.role === "B" ? session.role : null;
  const audioStudy = Boolean(publicConfig?.livekit?.enabled);
  const onlineReady = Boolean(
    publicConfig && session && bothPlayersConnected(session.presence) && (!audioStudy || voiceStatus.transcriptionReady)
  );
  const requiredConsentsAccepted = useMemo(() => {
    if (!publicConfig?.require_consent) return true;
    return publicConfig.consents.every((consent) => !consent.required || consentDecisions[consent.id] === true);
  }, [consentDecisions, publicConfig]);
  const dispatch = useCallback(
    (action: GameAction) => {
      if (session) {
        apiClient.sendAction(session.socket, action);
      }
    },
    [apiClient, session]
  );

  const refreshAudioInputs = useCallback(async () => {
    if (!navigator.mediaDevices?.enumerateDevices) return;
    const devices = await navigator.mediaDevices.enumerateDevices();
    const inputs = devices.filter((device) => device.kind === "audioinput");
    setAudioInputs(inputs);
    setSelectedAudioInputId((current) => current || inputs.find((device) => device.deviceId === "default")?.deviceId || inputs[0]?.deviceId || "");
  }, []);

  const connectRoom = useCallback((room: GameRoomResponse) => {
    const socket = new WebSocket(apiClient.socketUrl(room.room_id, room.participant_session_id));
    setSession({
      participantSessionId: room.participant_session_id,
      roomId: room.room_id,
      role: room.role as PlayerId | "spectator",
      state: room.state ?? null,
      observation: room.observation ?? room.state ?? initialState,
      availableActions: room.available_actions ?? [],
      eventLog: room.events ?? [],
      socket,
      connected: false,
      completed: false,
      presence: {},
      conversation: room.conversation ?? []
    });
    socket.addEventListener("open", () => {
      socket.send(JSON.stringify({ type: "ready" }));
      setSession((current) => (current?.socket === socket ? { ...current, connected: true } : current));
    });
    socket.addEventListener("message", (event) => {
      const message = JSON.parse(event.data) as GameServerMessage;
      if (message.type === "roleAssigned") {
        setSession((current) =>
          current?.socket === socket
            ? {
                ...current,
                role: message.role as PlayerId | "spectator",
                state: message.state ?? null,
                observation: message.observation ?? message.state ?? current.observation,
                availableActions: message.available_actions ?? [],
                eventLog: [...(message.events ?? []), ...current.eventLog].slice(0, 10),
                conversation: message.conversation ?? current.conversation,
                connected: true
              }
            : current
        );
      }
      if (message.type === "stateChanged") {
        setSession((current) =>
          current?.socket === socket
            ? {
                ...current,
                state: message.state ?? null,
                observation: message.observation ?? message.state ?? current.observation,
                availableActions: message.available_actions ?? [],
                eventLog: [...(message.events ?? []), ...current.eventLog].slice(0, 10),
                conversation: message.conversation ?? current.conversation
              }
            : current
        );
      }
      if (message.type === "presenceChanged") {
        setSession((current) =>
          current?.socket === socket ? { ...current, presence: normalizePresence(message.presence) } : current
        );
      }
      if (message.type === "completed") {
        setSession((current) => (current?.socket === socket ? { ...current, completed: true } : current));
      }
      if (message.type === "conversationMessageAdded") {
        setSession((current) =>
          current?.socket === socket
            ? { ...current, conversation: appendConversation(current.conversation, message.conversation_message) }
            : current
        );
      }
      if (message.type === "voiceStatusChanged") {
        const transcriptionMessage = message.voice?.transcriptionStatus;
        const voiceUpdate: Partial<VoiceStatus> = {};
        if (transcriptionMessage) {
          voiceUpdate.transcriptionMessage = transcriptionMessage;
        }
        if (typeof message.voice?.transcriptionReady === "boolean") {
          voiceUpdate.transcriptionReady = message.voice.transcriptionReady;
        }
        audioController.updateVoiceStatus(voiceUpdate);
      }
      if (message.type === "error") {
        setOnlineError(message.message ?? "Server rejected the last action.");
      }
    });
    socket.addEventListener("close", () => {
      setSession((current) => (current?.socket === socket ? { ...current, connected: false } : current));
    });
  }, [apiClient, audioController]);

  const connectMatchedDirect = useCallback(
    (match: GameMatchmakingResponse) => {
      if (!match.room_id || !match.role) {
        throw new Error("Matched response did not include a room assignment.");
      }
      connectRoom({
        room_id: match.room_id,
        participant_session_id: match.participant_session_id,
        role: match.role,
        state: match.state ?? null,
        observation: match.observation ?? match.state ?? initialState,
        available_actions: match.available_actions ?? [],
        events: match.events ?? [],
        conversation: match.conversation ?? []
      });
      setWaitingParticipantId(null);
    },
    [connectRoom]
  );

  const startOnlineRoom = useCallback(async () => {
    try {
      setOnlineError("");
      if (!requiredConsentsAccepted) {
        setOnlineError("Please accept all required consents before entering the waiting room.");
        return;
      }
      const participant = await apiClient.createParticipant(displayName);
      if (publicConfig?.require_consent) {
        await apiClient.submitConsent(participant.participant_session_id, consentDecisions);
      }
      const match = await apiClient.enterMatchmaking<StationState, StationObservation, GameAction, ObservationEvent>(
        participant.participant_session_id
      );
      if (match.status === "matched") {
        connectMatchedDirect(match);
      } else {
        setWaitingParticipantId(match.participant_session_id);
      }
    } catch (error) {
      setOnlineError(error instanceof Error ? error.message : "Could not enter the waiting room.");
    }
  }, [apiClient, connectMatchedDirect, consentDecisions, displayName, publicConfig, requiredConsentsAccepted]);

  useEffect(() => {
    let cancelled = false;
    apiClient.getPublicConfig()
      .then((config) => {
        if (!cancelled) setPublicConfig(config);
      })
      .catch((error) => {
        if (!cancelled) setOnlineError(error instanceof Error ? error.message : "Could not load experiment config.");
      });
    return () => {
      cancelled = true;
    };
  }, [apiClient]);

  useEffect(() => {
    void refreshAudioInputs();
    if (!navigator.mediaDevices?.addEventListener) return;
    const onDeviceChange = () => {
      void refreshAudioInputs();
    };
    navigator.mediaDevices.addEventListener("devicechange", onDeviceChange);
    return () => navigator.mediaDevices.removeEventListener("devicechange", onDeviceChange);
  }, [refreshAudioInputs]);

  useEffect(
    () => () => {
      void audioController.disconnect(true);
    },
    [audioController]
  );

  const leaveOnlineRoom = useCallback(() => {
    void audioController.disconnect(true);
    session?.socket.close();
    setSession(null);
    setWaitingParticipantId(null);
    setOnlineError("");
  }, [audioController, session]);

  const prepareVoice = useCallback(async () => {
    if (!publicConfig?.livekit?.enabled) return;
    setOnlineError("");
    try {
      await audioController.prepare(selectedAudioInputId, selectedAudioInputLabel(audioInputs, selectedAudioInputId));
      await refreshAudioInputs();
    } catch (error) {
      setOnlineError(error instanceof Error ? error.message : "Microphone permission was not granted.");
    }
  }, [audioController, audioInputs, publicConfig, refreshAudioInputs, selectedAudioInputId]);

  const toggleVoice = useCallback(async () => {
    if (!session) return;
    const selectedAudioInput = audioInputs.find((device) => device.deviceId === selectedAudioInputId);
    const logVoice = (event: string, metadata: Record<string, unknown> = {}) => {
      apiClient.postVoiceDiagnostic(session.roomId, session.participantSessionId, event, {
        role: session.role,
        selected_audio_input_id: selectedAudioInputId || null,
        selected_audio_input_label: selectedAudioInput?.label || null,
        ...metadata
      });
    };
    try {
      setOnlineError("");
      await audioController.toggle({
        roomId: session.roomId,
        participantSessionId: session.participantSessionId,
        role: session.role,
        selectedAudioInputId,
        selectedAudioInputLabel: selectedAudioInput?.label || null,
        getAudioSession: () => apiClient.getAudioSession(session.roomId, session.participantSessionId),
        getLiveKitToken: () => apiClient.getLiveKitToken(session.roomId, session.participantSessionId),
        postTranscriptSegment: (segment) => apiClient.postTranscriptSegment(session.roomId, segment),
        logVoice,
        onVoiceStatus: (status) => audioController.updateVoiceStatus(status)
      }, selectedAudioInputId, selectedAudioInputLabel(audioInputs, selectedAudioInputId));
    } catch (error) {
      setOnlineError(error instanceof Error ? error.message : "Could not start voice chat.");
    }
  }, [
    audioController,
    apiClient,
    audioInputs,
    selectedAudioInputId,
    session
  ]);

  useEffect(() => {
    return audioController.subscribe((snapshot) => {
      setVoiceStatus(snapshot.voiceStatus);
      setVoicePreflight(snapshot.voicePreflight);
    });
  }, [audioController]);

  const submitChat = useCallback(() => {
    const text = chatDraft.trim();
    if (!text || !session) return;
    apiClient.sendChatMessage(session.socket, text);
    setChatDraft("");
  }, [apiClient, chatDraft, session]);

  useEffect(() => {
    if (
      !session ||
      !publicConfig?.livekit?.enabled ||
      !voicePreflight.ready ||
      voiceStatus.connected ||
      voiceStatus.connecting
    ) {
      return;
    }
    void toggleVoice();
  }, [publicConfig, session, toggleVoice, voicePreflight.ready, voiceStatus.connected, voiceStatus.connecting]);

  useEffect(() => {
    if (!waitingParticipantId || session) return;
    let cancelled = false;
    const timer = window.setInterval(async () => {
      try {
        const match = await apiClient.waitForMatch<StationState, StationObservation, GameAction, ObservationEvent>(
          waitingParticipantId
        );
        if (!cancelled && match.status === "matched") {
          connectMatchedDirect(match);
        }
      } catch (error) {
        if (!cancelled) setOnlineError(error instanceof Error ? error.message : "Waiting room check failed.");
      }
    }, 1200);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [apiClient, connectMatchedDirect, session, waitingParticipantId]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (!session || !onlineReady) return;
      const target = event.target as HTMLElement | null;
      if (target?.closest("input, textarea, button")) return;
      const key = event.key.toLowerCase();
      const direction = movementKeys[key];
      if (direction && assignedRole) {
        event.preventDefault();
        dispatch({ type: "moveStep", player: assignedRole, direction });
        return;
      }

      if (key === "enter" && assignedRole) {
        const firstAction = session.availableActions[0] ?? availableActions(state, assignedRole)[0];
        if (firstAction) {
          event.preventDefault();
          dispatch(firstAction);
        }
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [assignedRole, dispatch, onlineReady, session, state]);

  const preGamePanel = waitingParticipantId ? (
    <QueueWaitingPanel
      audioInputs={audioInputs}
      liveKitEnabled={Boolean(publicConfig?.livekit?.enabled)}
      onLeave={() => setWaitingParticipantId(null)}
      onPrepareVoice={prepareVoice}
      onSelectedAudioInputChange={setSelectedAudioInputId}
      selectedAudioInputId={selectedAudioInputId}
      voicePreflight={voicePreflight}
    />
  ) : session && !onlineReady ? (
    <WaitingRoom
      liveKitEnabled={audioStudy}
      onLeave={leaveOnlineRoom}
      onToggleVoice={toggleVoice}
      session={session}
      voiceStatus={voiceStatus}
    />
  ) : !session ? (
      <LobbyPanel
        audioInputs={audioInputs}
        displayName={displayName}
        consentDecisions={consentDecisions}
        requiredConsentsAccepted={requiredConsentsAccepted}
        publicConfig={publicConfig}
        onDisplayNameChange={setDisplayName}
        onEnter={startOnlineRoom}
        onPrepareVoice={prepareVoice}
        onSelectedAudioInputChange={setSelectedAudioInputId}
        onConsentChange={(consentId, granted) =>
          setConsentDecisions((current) => ({ ...current, [consentId]: granted }))
        }
        onlineError={onlineError}
        selectedAudioInputId={selectedAudioInputId}
        voicePreflight={voicePreflight}
      />
  ) : null;

  return (
    <main className="app-shell">
      {session && onlineReady && (
        <section className="status-band" aria-label="Station systems">
          <div>
            <p className="eyebrow">Evacuation Beacon</p>
            <h1>{state.beaconLaunched ? "Carrier Lock Achieved" : "Station Repair"}</h1>
          </div>
          <SystemPill label="Power" online={systems.powerStable} />
          <SystemPill label="Oxygen" online={systems.oxygenStable} />
          <SystemPill label="Door" online={systems.doorAccess} />
          <SystemPill label="Signal" online={systems.signalRouted} />
          <SystemPill label="Cooling" online={systems.coolingRestored} />
        </section>
      )}

      {session && onlineReady && (
        <section className="session-band" aria-label="Online session controls">
          <div>
            <p className="eyebrow">Online direct room</p>
            <strong>{session.roomId}</strong>
            <span>
              {session.connected ? "Connected" : "Disconnected"} · You are Player {session.role}
              {session.completed && " · Complete"}
            </span>
          </div>
          <button onClick={leaveOnlineRoom}>Leave game</button>
          {onlineError && <p className="online-error">{onlineError}</p>}
        </section>
      )}

      {preGamePanel ?? (
        <section className={`game-layout ${assignedRole ? `game-layout-${assignedRole.toLowerCase()}` : ""}`}>
          {assignedRole === "A" && (
            <PlayerPanel
              canControl={onlineReady}
              player="A"
              state={state}
              actions={serverAvailableActions}
              dispatch={dispatch}
              setPreview={setPreview}
            />
          )}
          <section className="playfield-zone" aria-label="Top-down station playfield">
            <StationPlayfield preview={preview} state={state} systems={systems} />
            <SharedConsole preview={preview} state={state} systems={systems} eventLog={eventLog} />
            <CommunicationPanel
              chatDraft={chatDraft}
              conversation={session?.conversation ?? []}
              liveKitEnabled={audioStudy}
              onChatDraftChange={setChatDraft}
              onSubmitChat={submitChat}
              onToggleVoice={toggleVoice}
              voicePreflight={voicePreflight}
              voiceStatus={voiceStatus}
            />
          </section>
          {assignedRole === "B" && (
            <PlayerPanel
              canControl={onlineReady}
              player="B"
              state={state}
              actions={serverAvailableActions}
              dispatch={dispatch}
              setPreview={setPreview}
            />
          )}
        </section>
      )}
    </main>
  );
}

function LobbyPanel({
  audioInputs,
  displayName,
  consentDecisions,
  requiredConsentsAccepted,
  publicConfig,
  onDisplayNameChange,
  onConsentChange,
  onEnter,
  onPrepareVoice,
  onSelectedAudioInputChange,
  onlineError,
  selectedAudioInputId,
  voicePreflight
}: {
  audioInputs: MediaDeviceInfo[];
  displayName: string;
  consentDecisions: Record<string, boolean>;
  requiredConsentsAccepted: boolean;
  publicConfig: PublicConfigResponse | null;
  onDisplayNameChange: (value: string) => void;
  onConsentChange: (consentId: string, granted: boolean) => void;
  onEnter: () => void;
  onPrepareVoice: () => void;
  onSelectedAudioInputChange: (value: string) => void;
  onlineError: string;
  selectedAudioInputId: string;
  voicePreflight: VoicePreflight;
}) {
  const liveKitEnabled = Boolean(publicConfig?.livekit?.enabled);
  return (
    <section className="lobby-panel">
      <div className="lobby-heading">
        <p className="eyebrow">Cooperative Experiment</p>
        <h1>Station Repair</h1>
      </div>
      <div className="lobby-copy">
        <h2>Waiting Room</h2>
        <p>Review the consent statement, enter your name, and you will be paired automatically with the next player.</p>
        <p>When the game starts, use the arrow keys to move your character and Enter to use a device.</p>
      </div>
      <div className="lobby-actions">
        <input
          aria-label="Display name"
          onChange={(event) => onDisplayNameChange(event.target.value)}
          placeholder="Display name"
          value={displayName}
        />
        <button disabled={!requiredConsentsAccepted} onClick={onEnter}>
          Enter waiting room
        </button>
      </div>
      {publicConfig?.require_consent && (
        <div className="consent-list">
          {publicConfig.consents.map((consent) => (
            <label className="consent-row" key={consent.id}>
              <input
                checked={Boolean(consentDecisions[consent.id])}
                onChange={(event) => onConsentChange(consent.id, event.target.checked)}
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
      <VoicePreparationPanel
        audioInputs={audioInputs}
        liveKitEnabled={liveKitEnabled}
        onPrepareVoice={onPrepareVoice}
        onSelectedAudioInputChange={onSelectedAudioInputChange}
        selectedAudioInputId={selectedAudioInputId}
        voicePreflight={voicePreflight}
      />
      {onlineError && <p className="online-error">{onlineError}</p>}
    </section>
  );
}

function QueueWaitingPanel({
  audioInputs,
  liveKitEnabled,
  onLeave,
  onPrepareVoice,
  onSelectedAudioInputChange,
  selectedAudioInputId,
  voicePreflight
}: {
  audioInputs: MediaDeviceInfo[];
  liveKitEnabled: boolean;
  onLeave: () => void;
  onPrepareVoice: () => void;
  onSelectedAudioInputChange: (value: string) => void;
  selectedAudioInputId: string;
  voicePreflight: VoicePreflight;
}) {
  return (
    <section className="lobby-panel">
      <div className="lobby-heading">
        <p className="eyebrow">Cooperative Experiment</p>
        <h1>Station Repair</h1>
      </div>
      <div className="lobby-copy">
        <h2>Waiting for another player</h2>
        <p>Keep this tab open. The next player who arrives will be paired with you automatically.</p>
        <p>You will control only your own character. Use arrow keys to move and Enter to interact.</p>
      </div>
      <VoicePreparationPanel
        audioInputs={audioInputs}
        liveKitEnabled={liveKitEnabled}
        onPrepareVoice={onPrepareVoice}
        onSelectedAudioInputChange={onSelectedAudioInputChange}
        selectedAudioInputId={selectedAudioInputId}
        voicePreflight={voicePreflight}
      />
      <div className="lobby-actions">
        <button onClick={onLeave}>Leave waiting room</button>
      </div>
    </section>
  );
}

function VoicePreparationPanel({
  audioInputs,
  liveKitEnabled,
  onPrepareVoice,
  onSelectedAudioInputChange,
  selectedAudioInputId,
  voicePreflight
}: {
  audioInputs: MediaDeviceInfo[];
  liveKitEnabled: boolean;
  onPrepareVoice: () => void;
  onSelectedAudioInputChange: (value: string) => void;
  selectedAudioInputId: string;
  voicePreflight: VoicePreflight;
}) {
  return (
    <div className="voice-preflight">
      <div>
        <strong>Voice chat</strong>
        <span>{liveKitEnabled ? voicePreflight.message : "Voice is disabled for this study"}</span>
        <span className="mic-device-label">{voicePreflight.deviceLabel}</span>
      </div>
      <VoicePreparationControls
        audioInputs={audioInputs}
        liveKitEnabled={liveKitEnabled}
        onPrepareVoice={onPrepareVoice}
        onSelectedAudioInputChange={onSelectedAudioInputChange}
        selectedAudioInputId={selectedAudioInputId}
        voicePreflight={voicePreflight}
      />
    </div>
  );
}

function WaitingRoom({
  liveKitEnabled,
  onLeave,
  onToggleVoice,
  session,
  voiceStatus
}: {
  liveKitEnabled: boolean;
  onLeave: () => void;
  onToggleVoice: () => void;
  session: OnlineSession;
  voiceStatus: VoiceStatus;
}) {
  const aConnected = Boolean(session.presence.A?.connected);
  const bConnected = Boolean(session.presence.B?.connected);
  const playersConnected = aConnected && bConnected;
  const waitingForTranscription = liveKitEnabled && playersConnected && !voiceStatus.transcriptionReady;
  return (
    <section className="lobby-panel">
      <div className="lobby-heading">
        <p className="eyebrow">Cooperative Experiment</p>
        <h1>Station Repair</h1>
      </div>
      <div className="lobby-copy">
        <h2>{waitingForTranscription ? "Waiting for transcription service" : "Starting game"}</h2>
        <p>
          {liveKitEnabled
            ? "The game board unlocks when both players and the transcription service are ready."
            : "The game board unlocks when both players are connected."}
        </p>
        <p>Use arrow keys to move your assigned character. Press Enter to use a device.</p>
      </div>
      <div className="seat-grid">
        <div className={aConnected ? "seat-ready" : ""}>
          <strong>Player A</strong>
          <span>{aConnected ? "Connected" : "Waiting"}</span>
        </div>
        <div className={bConnected ? "seat-ready" : ""}>
          <strong>Player B</strong>
          <span>{bConnected ? "Connected" : "Waiting"}</span>
        </div>
        {liveKitEnabled && (
          <div className={voiceStatus.transcriptionReady ? "seat-ready" : ""}>
            <strong>Transcription Service</strong>
            <span>{voiceStatus.transcriptionReady ? "Ready" : voiceStatus.transcriptionMessage}</span>
          </div>
        )}
      </div>
      {liveKitEnabled && <TranscriptionProgress voiceStatus={voiceStatus} />}
      <div className="voice-preflight">
        <div>
          <strong>Voice chat</strong>
          <VoiceStatusChip liveKitEnabled={liveKitEnabled} voiceStatus={voiceStatus} />
        </div>
        <VoiceJoinButton liveKitEnabled={liveKitEnabled} onToggleVoice={onToggleVoice} voiceStatus={voiceStatus} />
      </div>
      <div className="lobby-actions">
        <button onClick={onLeave}>Leave waiting room</button>
      </div>
    </section>
  );
}

function PlayerPanel({
  canControl,
  player,
  state,
  actions,
  dispatch,
  setPreview
}: {
  canControl: boolean;
  player: PlayerId;
  state: StationState;
  actions: GameAction[];
  dispatch: (action: GameAction) => void;
  setPreview: (preview: ActionPreview | null) => void;
}) {
  const room = roomById[state.players[player].room];
  const controls = "Arrow keys + Enter";

  return (
    <aside className={`player-panel player-${player.toLowerCase()}`}>
      <div className="panel-heading">
        <p className="eyebrow">Player {player}</p>
        <h2>{room.name}</h2>
        <span className="control-hint">{controls}</span>
      </div>
      <div className="private-note">
        <span>Private schematic</span>
        <p>{playerBriefing(player)}</p>
      </div>
      <div className="movement-pad" aria-label={`Player ${player} movement controls`}>
        <button disabled={!canControl} onClick={() => dispatch({ type: "moveStep", player, direction: "up" })}>
          ↑
        </button>
        <button disabled={!canControl} onClick={() => dispatch({ type: "moveStep", player, direction: "left" })}>
          ←
        </button>
        <button disabled={!canControl} onClick={() => dispatch({ type: "moveStep", player, direction: "down" })}>
          ↓
        </button>
        <button disabled={!canControl} onClick={() => dispatch({ type: "moveStep", player, direction: "right" })}>
          →
        </button>
      </div>
      <div className="action-grid">
        {!canControl ? (
          <p className="no-actions">This browser is assigned to the other player.</p>
        ) : actions.length === 0 ? (
          <p className="no-actions">Stand on a device tile to use it. Floor plates work automatically.</p>
        ) : (
          actions.map((action, index) => (
            <button
              className={action.type === "launchBeacon" ? "critical-action" : ""}
              key={`${describeAction(action, state)}-${index}`}
              onClick={() => {
                setPreview(null);
                dispatch(action);
              }}
              onBlur={() => setPreview(null)}
              onFocus={() => setPreview(actionPreview(action, state))}
              onMouseEnter={() => setPreview(actionPreview(action, state))}
              onMouseLeave={() => setPreview(null)}
            >
              <span className="action-icon" aria-hidden="true">
                {actionIcon(action)}
              </span>
              <span>{describeAction(action, state)}</span>
            </button>
          ))
        )}
      </div>
      <section className="knowledge-list" aria-label={`Player ${player} discovered knowledge`}>
        <h3>Recent clues</h3>
        <ul>
          {state.knowledge[player].slice(-3).map((item) => (
            <li key={item}>{item}</li>
          ))}
        </ul>
      </section>
    </aside>
  );
}

interface OnlineSession {
  participantSessionId: string;
  roomId: string;
  role: PlayerId | "spectator";
  state: StationState | null;
  observation: StationObservation;
  availableActions: GameAction[];
  eventLog: ObservationEvent[];
  socket: WebSocket;
  connected: boolean;
  completed: boolean;
  presence: PresenceState;
  conversation: ConversationMessage[];
}

interface PresenceState {
  A?: { participantSessionId?: string; connected?: boolean };
  B?: { participantSessionId?: string; connected?: boolean };
}

function normalizePresence(presence: Record<string, unknown> | undefined): PresenceState {
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
    connected: Boolean(record.connected)
  };
}

function bothPlayersConnected(presence: PresenceState): boolean {
  return Boolean(presence.A?.connected && presence.B?.connected);
}

function appendConversation(current: ConversationMessage[], message: ConversationMessage): ConversationMessage[] {
  if (current.some((candidate) => candidate.id === message.id)) return current;
  return [...current, message].slice(-50);
}

function selectedAudioInputLabel(audioInputs: MediaDeviceInfo[], selectedAudioInputId: string): string {
  if (!selectedAudioInputId) return "Default microphone";
  return audioInputs.find((device) => device.deviceId === selectedAudioInputId)?.label || "Selected microphone";
}

function StationPlayfield({
  preview,
  state,
  systems
}: {
  preview: ActionPreview | null;
  state: StationState;
  systems: ReturnType<typeof deriveSystems>;
}) {
  const visibleDevices = devicesForState(state);

  return (
    <div
      className="station-playfield"
      style={{
        gridTemplateColumns: `repeat(${mapWidth}, minmax(0, 1fr))`,
        gridTemplateRows: `repeat(${mapHeight}, minmax(0, 1fr))`
      }}
    >
      {roomRegions.map((room) => (
        <div
          className={`room-region region-${room.id} ${roomEffectClass(room.id, systems, state)} ${
            preview?.rooms.includes(room.id) ? "preview-target" : ""
          }`}
          key={room.id}
          style={{
            gridColumn: `${room.left + 1} / ${room.right + 2}`,
            gridRow: `${room.top + 1} / ${room.bottom + 2}`
          }}
        >
          <span>{room.name}</span>
        </div>
      ))}
      {doors.map((door) => (
        <div
          aria-label={door.label}
          className={[
            "door",
            door.a.x === door.b.x ? "door-vertical" : "door-horizontal",
            door.kind === "pressure" && !systems.doorAccess ? "door-locked" : "door-open"
          ].join(" ")}
          key={door.id}
          style={{
            gridColumn: `${Math.min(door.a.x, door.b.x) + 1} / ${Math.max(door.a.x, door.b.x) + 2}`,
            gridRow: `${Math.min(door.a.y, door.b.y) + 1} / ${Math.max(door.a.y, door.b.y) + 2}`
          }}
          title={door.label}
        />
      ))}
      {cells().map((cell) => {
        const room = roomAtPosition(cell);
        const device = visibleDevices.find((candidate) => samePosition(candidate.position, cell));
        const players = (["A", "B"] as PlayerId[]).filter((player) =>
          samePosition(state.players[player].position, cell)
        );
        const deviceIsNearby = device && isNearPlayer(device.position, state);
        const deviceIsUnderPlayer = device && isPlayerOn(device.position, state);
        const sealedAirlock =
          room === "airlock" && !systems.doorAccess && (cell.x === 5 || cell.x === 7 || cell.y === 8);

        return (
          <div
            className={[
              "map-cell",
              room ? "floor" : "void",
              room ? `cell-${room}` : "",
              device ? "has-device" : "",
              state.visualEffects.includes(`room:${room}`) ? "effect-flash" : "",
              deviceIsNearby ? "device-nearby" : "",
              deviceIsUnderPlayer ? "device-ready" : "",
              sealedAirlock ? "sealed" : ""
            ].join(" ")}
            key={`${cell.x}-${cell.y}`}
            style={{ gridColumn: cell.x + 1, gridRow: cell.y + 1 }}
          >
            {room && <span className="floor-grid" aria-hidden="true" />}
            {device && (
              <DeviceSprite
                device={device}
                isPreviewed={Boolean(preview?.devices.includes(device.id))}
                state={state}
                systems={systems}
              />
            )}
            {players.map((player) => (
              <span key={player} className={`player-token token-${player.toLowerCase()}`}>
                {player}
              </span>
            ))}
          </div>
        );
      })}
    </div>
  );
}

function DeviceSprite({
  device,
  isPreviewed,
  state,
  systems
}: {
  device: DeviceDefinition;
  isPreviewed: boolean;
  state: StationState;
  systems: ReturnType<typeof deriveSystems>;
}) {
  return (
    <span
      className={`device device-${device.kind} device-${device.id} ${deviceStateClass(device, state, systems)} ${
        state.visualEffects.includes(`device:${device.id}`) ? "effect-flash" : ""
      } ${isPreviewed ? "preview-target" : ""}`}
      title={device.label}
      aria-label={device.label}
    >
      {deviceGlyph(device, state)}
    </span>
  );
}

function SharedConsole({
  eventLog,
  preview,
  state,
  systems
}: {
  eventLog: ObservationEvent[];
  preview: ActionPreview | null;
  state: StationState;
  systems: ReturnType<typeof deriveSystems>;
}) {
  const recentEvents =
    eventLog.length > 0
      ? eventLog.map((event) => event.text ?? event.type)
      : ["Emergency lamps are on. The beacon checklist is dark: power, oxygen, access, signal."];
  return (
    <section className="console-grid simplified-console">
      <div className="objective-card">
        <p className="eyebrow">{preview ? "Action preview" : "Goal"}</p>
        <h3>{preview?.title ?? (systems.readyToLaunch ? "Launch the beacon" : "Restore all systems")}</h3>
        <p>
          {preview?.summary ??
            "Hover an action to light up what it will affect. Stand on the floor plate to hold the hatch latch."}
        </p>
      </div>
      <div className="event-log">
        <h3>Recent events</h3>
        <ol>
          {recentEvents.slice(0, 4).map((line, index) => (
            <li key={`${line}-${index}`}>{line}</li>
          ))}
        </ol>
      </div>
    </section>
  );
}

function CommunicationPanel({
  chatDraft,
  conversation,
  liveKitEnabled,
  onChatDraftChange,
  onSubmitChat,
  onToggleVoice,
  voicePreflight,
  voiceStatus
}: {
  chatDraft: string;
  conversation: ConversationMessage[];
  liveKitEnabled: boolean;
  onChatDraftChange: (value: string) => void;
  onSubmitChat: () => void;
  onToggleVoice: () => void;
  voicePreflight: VoicePreflight;
  voiceStatus: VoiceStatus;
}) {
  const textMessages = conversation.filter((message) => message.origin !== "voice_transcript").slice(-6);
  if (!liveKitEnabled) {
    return (
      <section className="communication-panel" aria-label="Text chat">
        <div className="communication-header">
          <div>
            <p className="eyebrow">Comms</p>
            <h3>Text chat</h3>
          </div>
        </div>
        <ol className="conversation-list">
          {textMessages.length === 0 ? (
            <li className="conversation-empty">No chat yet.</li>
          ) : (
            textMessages.map((message) => (
              <li className={`conversation-message origin-${message.origin}`} key={message.id}>
                <span>{message.sender_role ?? labelForOrigin(message.origin)}</span>
                <p>{message.text}</p>
              </li>
            ))
          )}
        </ol>
        <form
          className="chat-form"
          onSubmit={(event) => {
            event.preventDefault();
            onSubmitChat();
          }}
        >
          <input
            aria-label="Text chat message"
            onChange={(event) => onChatDraftChange(event.target.value)}
            placeholder="Type to your partner"
            value={chatDraft}
          />
          <button disabled={!chatDraft.trim()} type="submit">
            Send
          </button>
        </form>
      </section>
    );
  }

  return (
    <section className="communication-panel" aria-label="Voice chat">
      <div className="communication-header">
        <div>
          <p className="eyebrow">Comms</p>
          <h3>{voiceStatus.message}</h3>
        </div>
        <VoiceJoinButton liveKitEnabled={liveKitEnabled} onToggleVoice={onToggleVoice} voiceStatus={voiceStatus} />
      </div>
      <div className="voice-feedback" aria-label="Voice diagnostics">
        <div className="meter-stack">
          <span className="mic-device-label">{voicePreflight.deviceLabel}</span>
          <MicLevelMeter active={voicePreflight.micProbeActive} label="Device" level={voicePreflight.micLevel} />
        </div>
        <TranscriptionStatusChip voiceStatus={voiceStatus} />
      </div>
    </section>
  );
}

function labelForOrigin(origin: ConversationMessage["origin"]): string {
  if (origin === "voice_transcript") return "Voice";
  if (origin === "agent") return "Agent";
  if (origin === "system") return "System";
  return "Text";
}

function SystemPill({ label, online }: { label: string; online: boolean }) {
  return (
    <div className={`system-pill ${online ? "online" : "offline"}`}>
      <span aria-hidden="true" />
      {label}
    </div>
  );
}

interface ActionPreview {
  title: string;
  summary: string;
  devices: string[];
  rooms: string[];
}

function samePosition(a: Position, b: Position): boolean {
  return a.x === b.x && a.y === b.y;
}

function playerBriefing(player: PlayerId): string {
  if (player === "A") {
    return "Electrical sheet: blue feeds the bus, yellow feeds the pump, AUX can backfeed the pump line.";
  }
  return "Flow sheet: valve A refills pressure, valve C trades charger water against coolant flow.";
}

function actionPreview(action: GameAction, state: StationState): ActionPreview {
  switch (action.type) {
    case "toggleFuse": {
      const devices = [`fuse-${action.color}`];
      const rooms = ["power"];
      const inserts = !state.fuses[action.color];
      if (action.color === "yellow" && inserts && state.breakers.aux) rooms.push("oxygen");
      return {
        title: `${inserts ? "Insert" : "Remove"} ${action.color} fuse`,
        summary:
          action.color === "blue"
            ? "Power Bay bus lights are affected."
            : action.color === "yellow"
              ? "Pump power changes; Oxygen may react if AUX is on."
              : "Reserve fuse changes only this rack.",
        devices,
        rooms
      };
    }
    case "toggleBreaker": {
      const turnsOn = !state.breakers[action.breaker];
      return {
        title: `Turn ${action.breaker.toUpperCase()} ${turnsOn ? "on" : "off"}`,
        summary:
          action.breaker === "main"
            ? "Main power lighting and battery charger feed are affected."
            : "AUX can backfeed the pump line if yellow fuse is inserted.",
        devices: [`breaker-${action.breaker}`],
        rooms: action.breaker === "main" ? ["power", "charger"] : ["power", "oxygen"]
      };
    }
    case "setValve":
      return {
        title: `${action.open ? "Open" : "Close"} valve ${action.valve}`,
        summary:
          action.valve === "A"
            ? "Cabin pressure in Oxygen changes."
            : action.valve === "C"
              ? "Water/coolant route changes between Charger and Valve Room."
              : "Coolant comes online, but pressure can drain.",
        devices: [`valve-${action.valve.toLowerCase()}`],
        rooms: action.valve === "A" ? ["valve", "oxygen"] : ["valve", "charger"]
      };
    case "holdOverride":
      return {
        title: `${action.held ? "Hold" : "Release"} bypass`,
        summary: "The airlock door motor stays powered while the battery is away.",
        devices: ["bypass"],
        rooms: ["junction", "airlock"]
      };
    case "chargeBattery":
      return {
        title: "Start battery charger",
        summary: "The charger and battery will visibly light if power, pump, and water are ready.",
        devices: ["charger", "battery"],
        rooms: ["charger"]
      };
    case "moveBattery":
      return {
        title: state.battery.location === "charger" ? "Move battery to Signal Array" : "Return battery to Charger",
        summary: "The BAT tile moves rooms; the airlock may lose motor power unless bypass is held.",
        devices: ["battery"],
        rooms: state.battery.location === "charger" ? ["charger", "signal", "airlock"] : ["signal", "charger"]
      };
    case "cycleRelay":
      return {
        title: `Rotate relay to ${nextRelayLabel(state)}`,
        summary: "The Signal Array routing label changes on the relay tile.",
        devices: ["relay"],
        rooms: ["signal"]
      };
    case "launchBeacon":
      return {
        title: "Launch beacon",
        summary: "Airlock and Signal Array react. If systems are not ready, the battery pulse is spent.",
        devices: ["beacon", "battery"],
        rooms: ["airlock", "signal"]
      };
    default:
      return {
        title: describeAction(action, state),
        summary: "This affects the highlighted object or room.",
        devices: [],
        rooms: []
      };
  }
}

function nextRelayLabel(state: StationState): string {
  if (state.relay === "bypass") return "LOOP";
  if (state.relay === "loop") return "ARRAY";
  return "BYPASS";
}

function isNearPlayer(position: Position, state: StationState): boolean {
  return (["A", "B"] as PlayerId[]).some((player) => {
    const playerPosition = state.players[player].position;
    return Math.abs(playerPosition.x - position.x) + Math.abs(playerPosition.y - position.y) <= 1;
  });
}

function isPlayerOn(position: Position, state: StationState): boolean {
  return (["A", "B"] as PlayerId[]).some((player) => samePosition(state.players[player].position, position));
}

function devicesForState(state: StationState): DeviceDefinition[] {
  return devices.map((device) => {
    if (device.id === "battery" && state.battery.location === "signal") {
      return { ...device, room: "signal", position: { x: 8, y: 8 } };
    }
    return device;
  });
}

function roomEffectClass(
  room: string,
  systems: ReturnType<typeof deriveSystems>,
  state: StationState
): string {
  if (room === "power" && systems.powerStable) return "room-powered";
  if (room === "oxygen" && systems.oxygenStable) return "room-oxygen";
  if (room === "oxygen" && state.oxygenFanTripped) return "room-warning";
  if (room === "charger" && systems.chargerFed) return "room-powered";
  if (room === "valve" && systems.coolingRestored) return "room-cooling";
  if (room === "signal" && systems.signalRouted) return "room-signal";
  if (room === "airlock" && systems.doorAccess) return "room-door";
  if (room === "airlock" && state.battery.spent) return "room-warning";
  return "";
}

function deviceGlyph(device: DeviceDefinition, state: StationState): string {
  if (device.id === "fuse-blue") return "BLU";
  if (device.id === "fuse-yellow") return "YEL";
  if (device.id === "fuse-red") return "RED";
  if (device.id === "breaker-main") return "MAIN";
  if (device.id === "breaker-aux") return "AUX";
  if (device.id === "valve-a") return state.valves.A ? "OPEN" : "SHUT";
  if (device.id === "valve-c") return state.valves.C ? "WATR" : "COOL";
  if (device.id === "valve-floodgate") return state.valves.floodgate ? "OPEN" : "SHUT";
  if (device.kind === "bypass") return state.overrideHeld ? "HELD" : "PULL";
  if (device.kind === "plate") return "▣";
  if (device.kind === "charger") return state.battery.charged ? "DONE" : "CHG";
  if (device.kind === "battery") return state.battery.charged ? "BAT+" : "BAT";
  if (device.kind === "relay") {
    if (state.relay === "bypass") return "BYP";
    if (state.relay === "array") return "ARR";
    return "LOOP";
  }
  if (device.kind === "diagnostic") return "?";
  if (device.kind === "beacon") return state.beaconLaunched ? "SENT" : "LAUN";
  return "•";
}

function deviceStateClass(
  device: DeviceDefinition,
  state: StationState,
  systems: ReturnType<typeof deriveSystems>
): string {
  if (device.id === "fuse-blue") return state.fuses.blue ? "active" : "";
  if (device.id === "fuse-yellow") return state.fuses.yellow ? "active yellow" : "";
  if (device.id === "fuse-red") return state.fuses.red ? "active red" : "";
  if (device.id === "breaker-main") return state.breakers.main ? "active" : "";
  if (device.id === "breaker-aux") return state.breakers.aux ? "active yellow" : "";
  if (device.id === "valve-a") return state.valves.A ? "active teal" : "";
  if (device.id === "valve-c") return state.valves.C ? "active teal" : "";
  if (device.id === "valve-floodgate") return state.valves.floodgate ? "active teal" : "";
  if (device.id === "bypass") return state.overrideHeld ? "active yellow" : "";
  if (device.id === "plate") return state.players.B.plateHeld ? "active yellow" : "";
  if (device.id === "charger") return systems.chargerFed ? "active" : "";
  if (device.id === "battery") return state.battery.charged ? "active yellow" : "";
  if (device.id === "relay") return state.relay === "array" ? "active green" : state.relay === "loop" ? "active yellow" : "";
  if (device.id === "beacon") return state.beaconLaunched ? "active green" : "";
  return "";
}

function actionIcon(action: GameAction): string {
  const icons: Record<GameAction["type"], string> = {
    moveStep: "↕",
    move: "↗",
    toggleFuse: "▮",
    toggleBreaker: "⏻",
    setValve: "◌",
    holdOverride: "⫷",
    togglePlate: "▣",
    chargeBattery: "⚡",
    moveBattery: "⇥",
    setRelay: "⌁",
    cycleRelay: "⌁",
    runDiagnostic: "?",
    launchBeacon: "▲",
    reset: "↺"
  };
  return icons[action.type];
}
