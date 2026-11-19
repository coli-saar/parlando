import { ParticipantConnection, type LiveSession } from "./participantConnection.js";
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
import { experimentAllowsIntake, microphoneMuteButtonLabel, requiredConsentsAccepted } from "./helpers.js";
import { MicrophoneLevelMeter, TranscriptionProgress } from "./voiceComponents.js";
import {
  ParticipantClient,
  type ExperimentInfo,
  type ParticipantOutcome,
  type RecruitmentHandoff,
  type PlayerMessage,
  type PlayerRole,
  type Presence
} from "./protocol.js";

export interface GameSnapshot<TObservation, TAction> {
  sessionId: string;
  role: PlayerRole;
  observation: TObservation;
  /** Most recent accepted action, or null for an initial or resynchronized observation. */
  transition: GameTransition<TAction> | null;
  availableActions: TAction[] | null;
  interactionEnabled: boolean;
  act(action: TAction): void;
}

/** One accepted action made observable to both player roles. */
export interface GameTransition<TAction> {
  actor: PlayerRole;
  action: TAction;
}

export interface ParticipantAppProps<TObservation, TAction, TCompletion = Record<string, unknown>> {
  renderGame(snapshot: GameSnapshot<TObservation, TAction>): ReactNode;
  /** Renders game-specific public success content inside the standard terminal shell. */
  renderCompletion?(completion: TCompletion): ReactNode;
  baseUrl?: string;
}

/** One authoritative point on a reconstructed session timeline. */
export interface ReplayFrame<TObservation, TAction, TCompletion = Record<string, unknown>> {
  event_index: number;
  game_time_ms: number;
  observation: TObservation;
  available_actions: TAction[] | null;
  transition: GameTransition<TAction> | null;
  conversation: PlayerMessage[];
  completion: TCompletion | null;
}

/** Complete administrator replay payload containing both role-specific views. */
export interface SessionReplay<TObservation, TAction, TCompletion = Record<string, unknown>> {
  experiment_id: string;
  session_id: number;
  public_session_id: string;
  compatibility: { state: string; action: string; rules: string };
  roles: Record<PlayerRole, ReplayFrame<TObservation, TAction, TCompletion>[]>;
}

interface ParticipantAppRuntimeProps<TObservation, TAction, TCompletion = Record<string, unknown>> {
  renderGame(snapshot: GameSnapshot<TObservation, TAction>): ReactNode;
  renderCompletion?(completion: TCompletion): ReactNode;
  apiClient: ParticipantClient;
  createAudioController: () => AudioSessionController;
}

/** Renders the standard participant experience with the game's own view. */
export function ParticipantApp<
  TObservation,
  TAction = unknown,
  TCompletion = Record<string, unknown>
>({ renderGame, renderCompletion, baseUrl }: ParticipantAppProps<TObservation, TAction, TCompletion>) {
  const apiClient = useMemo(() => new ParticipantClient({ baseUrl }), [baseUrl]);
  const replayUrl = replayRequestUrl();
  if (replayUrl) {
    return <ReplayApp replayUrl={replayUrl} renderGame={renderGame} />;
  }
  return (
    <ParticipantAppRuntime
      apiClient={apiClient}
      createAudioController={createDefaultAudioController}
      renderGame={renderGame}
      renderCompletion={renderCompletion}
    />
  );
}

/** Reads and validates the administrator replay endpoint requested by the dashboard. */
function replayRequestUrl(): string | null {
  if (typeof window === "undefined") return null;
  const value = new URLSearchParams(window.location.search).get("parlandoReplay");
  if (!value) return null;
  const url = new URL(value, window.location.origin);
  if (url.origin !== window.location.origin || !url.pathname.startsWith("/api/admin/")) return null;
  return `${url.pathname}${url.search}`;
}

/** Loads an administrator replay without entering any participant lifecycle. */
function ReplayApp<TObservation, TAction, TCompletion>({
  replayUrl,
  renderGame
}: {
  replayUrl: string;
  renderGame(snapshot: GameSnapshot<TObservation, TAction>): ReactNode;
}) {
  const [replay, setReplay] = useState<SessionReplay<TObservation, TAction, TCompletion> | null>(null);
  const [error, setReplayError] = useState("");
  useEffect(() => {
    let active = true;
    void fetch(replayUrl)
      .then(async (response) => {
        if (!response.ok) throw new Error(await response.text());
        return response.json() as Promise<SessionReplay<TObservation, TAction, TCompletion>>;
      })
      .then((value) => { if (active) setReplay(value); })
      .catch((caught) => { if (active) setReplayError(errorMessage(caught, "Could not load replay.")); });
    return () => { active = false; };
  }, [replayUrl]);
  if (error) return <section className="parlando-replay-error" role="alert">{error}</section>;
  if (!replay) return <section className="parlando-replay-loading" role="status">Loading replay…</section>;
  return <ReplayPlayer replay={replay} renderGame={renderGame} />;
}

/** Renders one recorded session through the same game surface used during live play. */
export function ReplayPlayer<TObservation, TAction, TCompletion = Record<string, unknown>>({
  replay,
  renderGame
}: {
  replay: SessionReplay<TObservation, TAction, TCompletion>;
  renderGame(snapshot: GameSnapshot<TObservation, TAction>): ReactNode;
}) {
  const [role, setRole] = useState<PlayerRole>("A");
  const [position, setPosition] = useState(0);
  const [playing, setPlaying] = useState(false);
  const frames = replay.roles[role] ?? [];
  const safePosition = Math.min(position, Math.max(0, frames.length - 1));
  const frame = frames[safePosition];
  useEffect(() => {
    if (!playing || !frame || safePosition >= frames.length - 1) {
      if (playing && safePosition >= frames.length - 1) setPlaying(false);
      return;
    }
    const delay = Math.max(100, frames[safePosition + 1].game_time_ms - frame.game_time_ms);
    const timer = window.setTimeout(() => setPosition((current) => current + 1), delay);
    return () => window.clearTimeout(timer);
  }, [frame, frames, playing, safePosition]);
  if (!frame) return <section className="parlando-replay-error" role="alert">This replay has no frames.</section>;
  const snapshot: GameSnapshot<TObservation, TAction> = {
    sessionId: replay.public_session_id,
    role,
    observation: frame.observation,
    transition: frame.transition,
    availableActions: frame.available_actions,
    interactionEnabled: false,
    act: () => undefined
  };
  return (
    <div className="parlando-replay">
      <style>{`.parlando-replay{min-height:100%;padding-bottom:6.5rem}.parlando-replay-conversation{position:fixed;z-index:999;top:1rem;right:1rem;display:grid;gap:.4rem;width:min(20rem,calc(100vw - 2rem));max-height:35vh;overflow:auto;margin:0;padding:.75rem;list-style:none;border:1px solid var(--parlando-replay-border,#68707c);border-radius:.8rem;background:var(--parlando-replay-background,rgba(20,24,31,.96));color:var(--parlando-replay-color,#fff)}.parlando-replay-message{display:grid;grid-template-columns:auto 1fr;gap:.5rem}.parlando-replay-message p{margin:0}.parlando-replay-controls{position:fixed;z-index:1000;left:50%;bottom:1rem;transform:translateX(-50%);display:grid;grid-template-columns:auto auto auto minmax(12rem,32rem) auto;gap:.6rem;align-items:center;width:min(54rem,calc(100vw - 2rem));padding:.8rem 1rem;border:1px solid var(--parlando-replay-border,#68707c);border-radius:.8rem;background:var(--parlando-replay-background,rgba(20,24,31,.96));color:var(--parlando-replay-color,#fff);box-shadow:0 .6rem 2rem rgba(0,0,0,.3)}.parlando-replay-controls button,.parlando-replay-controls select{font:inherit}.parlando-replay-controls input{width:100%}.parlando-replay-time{font-variant-numeric:tabular-nums;white-space:nowrap}@media(max-width:640px){.parlando-replay-controls{grid-template-columns:auto auto auto 1fr;}.parlando-replay-controls select{grid-column:1/-1}.parlando-replay-time{display:none}}`}</style>
      <div aria-disabled="true" className="parlando-replay-surface" inert>{renderGame(snapshot)}</div>
      {frame.conversation.length > 0 && <ol className="parlando-replay-conversation" aria-label="Conversation at this event">
        {frame.conversation.map((message) => <li className="parlando-replay-message" key={message.id}><strong>{message.sender}</strong><p>{message.text}</p></li>)}
      </ol>}
      <nav className="parlando-replay-controls" aria-label="Replay timeline">
        <button aria-label="Previous event" disabled={safePosition === 0} onClick={() => { setPlaying(false); setPosition(safePosition - 1); }} type="button">←</button>
        <button aria-label={playing ? "Pause replay" : "Play replay"} onClick={() => { if (safePosition >= frames.length - 1) setPosition(0); setPlaying((value) => !value); }} type="button">{playing ? "Pause" : "Play"}</button>
        <button aria-label="Next event" disabled={safePosition >= frames.length - 1} onClick={() => { setPlaying(false); setPosition(safePosition + 1); }} type="button">→</button>
        <input aria-label="Replay event" max={Math.max(0, frames.length - 1)} min={0} onChange={(event) => { setPlaying(false); setPosition(Number(event.target.value)); }} type="range" value={safePosition} />
        <span className="parlando-replay-time">{Math.round(frame.game_time_ms / 100) / 10}s · {safePosition + 1}/{frames.length}</span>
        <select aria-label="Replay viewpoint" onChange={(event) => setRole(event.target.value as PlayerRole)} value={role}>
          <option value="A">Player A view</option>
          <option value="B">Player B view</option>
        </select>
      </nav>
    </div>
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
  const [consentDeclined, setConsentDeclined] = useState(false);
  const [session, setSession] = useState<LiveSession<TObservation, TAction, TCompletion> | null>(null);
  const [error, setError] = useState("");
  const [audioInputs, setAudioInputs] = useState<MediaDeviceInfo[]>([]);
  const [selectedAudioInputId, setSelectedAudioInputId] = useState("");
  const [status, setVoiceStatus] = useState<VoiceStatus>(initialVoiceStatus);
  const [voicePreflight, setVoicePreflight] = useState<VoicePreflight>(initialVoicePreflight);
  const [voiceReconnectGeneration, setVoiceReconnectGeneration] = useState(0);
  const [entering, setEntering] = useState(false);
  const enteringRef = useRef(false);
  const connection = useMemo(() => new ParticipantConnection<TObservation, TAction, TCompletion>(apiClient, {
    change: setSession,
    error: setError,
    terminal: (snapshot) => { if (!farewellVoiceIsOpen(snapshot)) void audioController.disconnect(true); },
    voice: (voice) => audioController.updateVoiceStatus(voiceStatusUpdate(voice))
  }), [apiClient, audioController]);
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
    connection.stop();
    void audioController.disconnect(true);
  }, [audioController, connection]);

  /** Commits leave before releasing session transports. */
  const leave = useCallback(() => {
    setError("");
    void connection.leave().catch((caught) => setError(errorMessage(caught, "Could not record that you left the session.")));
  }, [connection]);

  const ensureParticipant = useCallback(async () => {
    if (!publicConfig) throw new Error("Experiment config has not loaded.");
    if (!requiredConsentsAccepted(publicConfig, consentDecisions)) {
      throw new Error("Please accept all required consents before entering the waiting room.");
    }
    if (!apiClient.hasCredential()) await apiClient.register();
    if (publicConfig.consents.length > 0) {
      await apiClient.acceptConsents(consentDecisions);
    }
  }, [apiClient, consentDecisions, publicConfig]);

  const createDirectRoom = useCallback(async () => {
    if (enteringRef.current) return;
    enteringRef.current = true;
    setEntering(true);
    try {
      setError("");
      await connection.join(ensureParticipant);
    } catch (caught) {
      setError(errorMessage(caught, "Could not create the waiting room."));
    } finally {
      enteringRef.current = false;
      setEntering(false);
    }
  }, [connection, ensureParticipant]);

  /** Retries only the game channel after the server has already admitted this participant. */
  const retryGameConnection = useCallback(async () => {
    setError("");
    try {
      await connection.retry();
    } catch (caught) {
      setError(errorMessage(caught, "Could not connect to the game channel."));
    }
  }, [connection]);

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

  /** Builds the current session-bound context shared by voice connection and mute operations. */
  const currentAudioContext = useCallback((): AudioSessionContext | null => {
    if (!session || session.participantState.state === "registered") return null;
    if (session.participantState.state === "ended" && !farewellVoiceIsOpen(session.participantState)) return null;
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
    if (!enteringRef.current) void connection.join().catch(() => {});
  }, [apiClient, connection, publicConfig, session]);

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
    if (session?.synchronization !== "connected" || session.participantState.state === "ended" || !enabled || !voicePreflight.ready || status.connected || status.connecting) return;
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
        farewellContent={result.outcome === "completed" && session.participantState.voice_ends_at ? (
          <FarewellVoiceNotice
            deadlineAt={session.participantState.voice_ends_at}
            onEnd={() => void audioController.disconnect(true)}
            onMutedChange={setMicrophoneMuted}
            status={status}
          />
        ) : null}
      />
    );
  }

  if (consentDeclined) {
    return (
      <StartupShell
        gameName={publicConfig.gameName}
        institution={publicConfig.institution}
        heading="Consent declined"
        body="You have not entered the experiment, and no participant record was created. You may close this page."
        error=""
      />
    );
  }

  if (session?.participantState.state === "active" || session?.participantState.state === "paused") {
    const participantState = session.participantState;
    const gameSnapshot: GameSnapshot<TObservation, TAction> = {
      sessionId: participantState.public_session_id,
      role: participantState.role,
      observation: participantState.observation,
      transition: session.transition,
      availableActions: participantState.available_actions,
      interactionEnabled: participantState.state === "active" && session.synchronization === "connected",
      act: (action) => connection.act(action)
    };
    return (
      <>
        <div className="parlando-game-surface">{renderGame(gameSnapshot)}</div>
        <SessionTools
          connected={session.synchronization === "connected"}
          conversation={session.conversation}
          interactionEnabled={gameSnapshot.interactionEnabled}
          onLeave={leave}
          onMessage={(text) => connection.message(text)}
          onMutedChange={setMicrophoneMuted}
          role={participantState.role}
          sessionId={participantState.public_session_id}
          voiceEnabled={enabled}
          voicePreflight={voicePreflight}
          voiceStatus={status}
        />
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
          {session.synchronization === "disconnected" && <button onClick={() => void retryGameConnection()}>Retry connection</button>}
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
        {publicConfig.recruitment?.provider === "prolific" ? (
          publicConfig.recruitment.decline_url ? (
            <a className="parlando-decline-consent" href={publicConfig.recruitment.decline_url}>
              Do not consent
            </a>
          ) : null
        ) : publicConfig.consents.length > 0 ? (
          <button className="parlando-decline-consent" onClick={() => setConsentDeclined(true)} type="button">
            Do not consent
          </button>
        ) : null}
      </div>
    </StartupShell>
  );
}

/** Renders transport, conversation, microphone, and leave controls owned by Parlando. */
function SessionTools({
  connected,
  conversation,
  interactionEnabled,
  onLeave,
  onMessage,
  onMutedChange,
  role,
  sessionId,
  voiceEnabled,
  voicePreflight,
  voiceStatus
}: {
  connected: boolean;
  conversation: PlayerMessage[];
  interactionEnabled: boolean;
  onLeave: () => void;
  onMessage: (text: string) => void;
  onMutedChange: (muted: boolean) => Promise<void>;
  role: PlayerRole;
  sessionId: string;
  voiceEnabled: boolean;
  voicePreflight: VoicePreflight;
  voiceStatus: VoiceStatus;
}) {
  const [draft, setDraft] = useState("");
  const muted = voiceStatus.connected && !voiceStatus.microphoneEnabled;
  const messages = conversation.filter((message) => message.input === "text").slice(-6);
  return (
    <section className="parlando-session-tools" aria-label="Session controls">
      <style>{`.parlando-session-tools{display:grid;gap:.75rem;padding:1rem;border:1px solid var(--parlando-tools-border,currentColor);border-radius:var(--parlando-tools-radius,.75rem);background:var(--parlando-tools-background,transparent);color:var(--parlando-tools-color,inherit)}.parlando-session-tools-header,.parlando-session-tools-actions{display:flex;align-items:center;justify-content:space-between;gap:.75rem}.parlando-session-tools-conversation{display:grid;gap:.35rem;max-height:10rem;overflow:auto;margin:0;padding:0;list-style:none}.parlando-session-tools-message{display:grid;grid-template-columns:auto 1fr;gap:.5rem}.parlando-session-tools-message p{margin:0}.parlando-session-tools-form{display:flex;gap:.5rem}.parlando-session-tools-form input{min-width:0;flex:1}.parlando-session-tools-voice{display:grid;gap:.4rem}`}</style>
      <header className="parlando-session-tools-header">
        <span><strong>Session {sessionId}</strong> · Player {role} · {connected ? "Connected" : "Reconnecting"}</span>
        <button className="parlando-leave-session" onClick={onLeave} type="button">Leave game</button>
      </header>
      <ol className="parlando-session-tools-conversation" aria-label="Text conversation" data-testid="messages">
        {messages.length === 0 ? <li className="parlando-session-tools-empty">No messages yet.</li> : messages.map((message) => (
          <li className="parlando-session-tools-message" key={message.id}><strong>{message.sender}</strong><p>{message.text}</p></li>
        ))}
      </ol>
      <form className="parlando-session-tools-form" onSubmit={(event) => { event.preventDefault(); const text = draft.trim(); if (!text) return; onMessage(text); setDraft(""); }}>
        <input aria-label="Message" disabled={!interactionEnabled} onChange={(event) => setDraft(event.target.value)} value={draft} />
        <button disabled={!interactionEnabled || !draft.trim()} type="submit">Send message</button>
      </form>
      {voiceEnabled && <div className={`parlando-session-tools-voice ${muted ? "muted" : "live"}`}>
        <div className="parlando-session-tools-actions">
          <strong>Microphone</strong>
          <button aria-pressed={muted} className={`microphone-mute-button ${muted ? "muted" : "live"}`} disabled={!voiceStatus.connected || voiceStatus.microphoneChanging} onClick={() => void onMutedChange(!muted)} type="button">{microphoneMuteButtonLabel(voiceStatus)}</button>
        </div>
        <MicrophoneLevelMeter active={voicePreflight.micProbeActive} label={voicePreflight.deviceLabel || "Level"} level={voicePreflight.micLevel} muted={muted} />
        <span data-testid="voice">{voiceStatus.message}</span>
        <span data-testid="remote-audio">{voiceStatus.remoteAudio ? "heard" : "silent"}</span>
        {voiceStatus.error && <p className="voice-error" role="alert">{voiceStatus.error}</p>}
      </div>}
    </section>
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
    ? "If no partner joins, or you leave now, the game will not start. You will return to Prolific and be asked to return your submission. The researcher can issue a partial payment for the time you waited."
    : "If no partner joins, or you leave now, this session will end without starting the game.";
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
  completionContent,
  farewellContent
}: {
  outcome: ParticipantOutcome | null;
  reason: string | null;
  handoff: RecruitmentHandoff | null;
  recruitment?: ExperimentInfo["recruitment"];
  completionContent?: ReactNode;
  farewellContent?: ReactNode;
}) {
  const heading = outcome === "completed" ? "Session complete" : "Session ended";
  return (
    <section className={`parlando-session-outcome outcome-${outcome ?? "unknown"}`}>
      <h1>{heading}</h1>
      <p>{outcomeText(outcome, reason)}</p>
      {completionContent && (
        <div className="parlando-game-completion">{completionContent}</div>
      )}
      {farewellContent}
      {handoff && (
        <ProlificHandoff handoff={handoff} />
      )}
    </section>
  );
}

/** Standard terminal notice for the bounded participant-to-participant voice period. */
export function FarewellVoiceNotice({
  deadlineAt,
  onEnd,
  onMutedChange,
  status
}: {
  deadlineAt: string;
  onEnd: () => void;
  onMutedChange: (muted: boolean) => void;
  status: VoiceStatus;
}) {
  const [now, setNow] = useState(Date.now());
  const endedRef = useRef(false);
  const remainingSeconds = Math.max(0, Math.ceil((Date.parse(deadlineAt) - now) / 1_000));
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, []);
  useEffect(() => {
    if (remainingSeconds > 0 || endedRef.current) return;
    endedRef.current = true;
    onEnd();
  }, [onEnd, remainingSeconds]);
  const muted = status.connected && !status.microphoneEnabled;
  return (
    <section className="parlando-farewell-voice">
      <strong>Voice chat remains open</strong>
      {remainingSeconds > 0 && status.connected ? (
        <>
          <span>{formatCountdown(remainingSeconds)} remaining</span>
          <span>You can talk with your partner until the timer reaches zero. This conversation is not transcribed or stored.</span>
          <div className="parlando-farewell-actions">
            <button
              aria-pressed={muted}
              className={`microphone-mute-button ${muted ? "muted" : "live"}`}
              disabled={status.microphoneChanging}
              onClick={() => onMutedChange(!muted)}
              type="button"
            >
              {microphoneMuteButtonLabel(status)}
            </button>
            <button onClick={onEnd} type="button">End voice chat</button>
          </div>
        </>
      ) : (
        <span>Voice chat has closed.</span>
      )}
    </section>
  );
}

/** Reports whether an ended participant snapshot still authorizes farewell voice. */
export function farewellVoiceIsOpen(
  state: { result: { outcome: ParticipantOutcome }; voice_ends_at?: string | null },
  now = Date.now()
): boolean {
  return state.result.outcome === "completed"
    && typeof state.voice_ends_at === "string"
    && Date.parse(state.voice_ends_at) > now;
}

/** Formats one non-negative countdown as minutes and two-digit seconds. */
function formatCountdown(seconds: number): string {
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
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
    case "left_waiting_room": return "You left before a playable game began. Use the Game did not start completion path below to return to Prolific.";
    case "left_game": return "You left after the game started. Your responses up to that point have been recorded.";
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

function selectedAudioInputLabel(audioInputs: MediaDeviceInfo[], selectedAudioInputId: string): string {
  if (!selectedAudioInputId) return "Default microphone";
  return audioInputs.find((device) => device.deviceId === selectedAudioInputId)?.label || "Selected microphone";
}

function errorMessage(caught: unknown, fallback: string): string {
  return caught instanceof Error ? caught.message : fallback;
}
