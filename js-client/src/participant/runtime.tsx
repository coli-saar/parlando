/**
 * ParticipantAppRuntime coordinates live registration, consent, room entry, and terminal views
 * for ParticipantApp. ParticipantClient handles HTTP, ParticipantConnection reconciles the
 * role-specific game state, and useParticipantVoice manages audio. The game renderer receives
 * the current observation and actions through GameSnapshot.
 */

import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { ParticipantConnection, type LiveSession } from "../participantConnection.js";
import { AudioSessionController } from "../audio/audioSessionController.js";
import { errorMessage, experimentAllowsIntake, requiredConsentsAccepted } from "../helpers.js";
import { TranscriptionProgress } from "../voiceComponents.js";
import { ParticipantClient, type ExperimentInfo } from "../protocol.js";
import { type CommunicationPlacement, type GameSnapshot } from "./types.js";
import { CommunicationContext, CommunicationWidget } from "./communicationWidget.js";
import { ParticipantConsentForm } from "./consent.js";
import { StudyFooter } from "./studyFooter.js";
import { SessionTools } from "./sessionControls.js";
import { WaitingRoomNotice, PartnerReconnectNotice, IdleDeadlineNotice } from "./deadlines.js";
import { SessionOutcomePanel, FarewellVoiceNotice } from "./outcome.js";
import { StartupShell, ReadinessBoard, normalizePresence } from "./lobby.js";
import { VoicePreparationControls, isVoiceEnabled, voiceStatusUpdate } from "./microphone.js";
import { farewellVoiceIsOpen, useParticipantVoice } from "./useParticipantVoice.js";

interface ParticipantAppRuntimeProps<TObservation, TAction, TCompletion = Record<string, unknown>> {
  renderGame(snapshot: GameSnapshot<TObservation, TAction>): ReactNode;
  renderCompletion?(completion: TCompletion): ReactNode;
  communicationPlacement?: CommunicationPlacement;
  apiClient: ParticipantClient;
  createAudioController: () => AudioSessionController;
}

/** Keeps public privacy and operator links available throughout the live participant flow.
 * The shared footer remains mounted across loading, consent, waiting, play, and outcome views.
 * A single column container isolates its placement from the host page’s flex/grid layout.
 * Games set --parlando-participant-max-width to size the session and footer together. */
export function ParticipantAppRuntime<TObservation, TAction = unknown, TCompletion = Record<string, unknown>>(
  props: ParticipantAppRuntimeProps<TObservation, TAction, TCompletion>
) {
  return (
    <div className="parlando-participant-app" style={{ display: "flex", flexDirection: "column", width: "100%", maxWidth: "var(--parlando-participant-max-width, 100%)", minWidth: 0, marginInline: "auto", boxSizing: "border-box" }}>
      <ParticipantSession {...props} />
      <StudyFooter
        participantInformationUrl={props.apiClient.informationPageUrl("participants")}
        privacyNoticeUrl={props.apiClient.informationPageUrl("privacy")}
        imprintUrl={props.apiClient.informationPageUrl("imprint")}
      />
    </div>
  );
}

/** Renders the server's participant state using the supplied game and audio transports.
 * A pending admission or leave keeps its current view until the server responds.
 * Reconnecting and paused sessions disable game interaction. */
function ParticipantSession<
  TObservation,
  TAction = unknown,
  TCompletion = Record<string, unknown>
>({ renderGame, renderCompletion, apiClient, createAudioController, communicationPlacement = "default" }: ParticipantAppRuntimeProps<TObservation, TAction, TCompletion>) {
  const audioControllerRef = useRef<AudioSessionController | null>(null);
  if (!audioControllerRef.current) audioControllerRef.current = createAudioController();
  const audioController = audioControllerRef.current;

  const [publicConfig, setPublicConfig] = useState<ExperimentInfo | null>(null);
  const [configLoading, setConfigLoading] = useState(true);
  const [consentDecisions, setConsentDecisions] = useState<Record<string, boolean>>({});
  const [consentDeclined, setConsentDeclined] = useState(false);
  const [session, setSession] = useState<LiveSession<TObservation, TAction, TCompletion> | null>(null);
  const [error, setError] = useState("");

  const [entering, setEntering] = useState(false);
  const enteringRef = useRef(false);
  const connection = useMemo(() => new ParticipantConnection<TObservation, TAction, TCompletion>(apiClient, {
    change: setSession,
    error: setError,
    terminal: (snapshot) => { if (!farewellVoiceIsOpen(snapshot)) void audioController.disconnect(true); },
    voice: (voice) => audioController.updateVoiceStatus(voiceStatusUpdate(voice))
  }), [apiClient, audioController]);

  const consentReady = requiredConsentsAccepted(publicConfig, consentDecisions);
  const enabled = isVoiceEnabled(publicConfig);

  const {
    audioInputs, selectedAudioInputId, status, voicePreflight,
    cancelPreparation, prepareVoice, setMicrophoneMuted
  } = useParticipantVoice(audioController, apiClient, session, enabled, setError);

  const canEnter = Boolean(
    !entering && experimentAllowsIntake(publicConfig?.status) && consentReady && (!enabled || voicePreflight.ready)
  );

  /** Cancels in-flight preparation and shuts down both participant transports. */
  const endCurrentSession = useCallback(() => {
    cancelPreparation();
    connection.stop();
    void audioController.disconnect(true);
  }, [audioController, connection, cancelPreparation]);

  /** Commits leave before releasing session transports. */
  const leave = useCallback(() => {
    setError("");
    void connection.leave().catch((caught) => setError(errorMessage(caught, "Could not record that you left the session.")));
  }, [connection]);

  /** Establishes participant credentials and records the configured consent decisions. */
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

  /** Serializes entry requests while the server allocates or recovers the session. */
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
      recruitmentSource: participantState.recruitment_source ?? null,
      observation: participantState.observation,
      transition: session.transition,
      availableActions: participantState.available_actions,
      interactionEnabled: participantState.state === "active" && session.synchronization === "connected",
      act: (action) => connection.act(action)
    };
    return (
      <CommunicationContext.Provider value={enabled ? {
        mode: "speech", preflight: voicePreflight, status, setMuted: setMicrophoneMuted
      } : {
        mode: "typed", conversation: session.conversation, enabled: gameSnapshot.interactionEnabled,
        send: (message) => connection.message(message)
      }}>
        <div className="parlando-game-surface">{renderGame(gameSnapshot)}</div>
        {communicationPlacement === "default" && <CommunicationWidget />}
        <SessionTools
          connected={session.synchronization === "connected"}
          onLeave={leave}
          role={participantState.role}
          sessionId={participantState.public_session_id}
        />
        <IdleDeadlineNotice deadlineAt={participantState.idle_deadline_at} />
        {participantState.state === "paused" && participantState.reason.type === "partner_reconnecting" && <PartnerReconnectNotice deadlineAt={participantState.reason.deadline_at} />}
        {error && <p className="online-error">{error}</p>}
      </CommunicationContext.Provider>
    );
  }

  // A resumed waiting room has a seat credential but needs to prepare this page's microphone.
  const microphoneControls = (
    <VoicePreparationControls
      audioInputs={audioInputs}
      allowDeviceChange={!session}
      enabled={enabled}
      onPrepareVoice={prepareVoice}
      selectedAudioInputId={selectedAudioInputId}
      voicePreflight={voicePreflight}
    />
  );

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
            prolific={session.participantState.recruitment_source === "prolific"}
            startedAt={session.participantState.waiting_started_at}
          />
        )}
        <ReadinessBoard presence={presence} role={session.participantState.state === "waiting" ? session.participantState.role : undefined} />
        {enabled && (
          <div className="voice-preflight">
            <div><strong>Voice chat</strong>{" "}<span>{status.message}</span></div>
            {microphoneControls}
            <details>
              <summary>Voice connection details</summary>
              <TranscriptionProgress connected={session.synchronization === "connected"} status={status} />
            </details>
          </div>
        )}
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
      institution={publicConfig.institution}
      heading={publicConfig.gameName || "Join experiment"}
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
            <ParticipantConsentForm
              key={`${consent.id}:${consent.body}`}
              consent={consent}
              onChange={(accepted) => setConsentDecisions((current) => ({ ...current, [consent.id]: accepted }))}
            />
          ))}
        </div>
      )}
      {enabled && (
        <div className="voice-preflight">
          <div>
            <strong>Voice chat</strong>
            <span>{voicePreflight.message}</span>
          </div>
          {microphoneControls}
        </div>
      )}
      <div className="lobby-actions lobby-entry-actions">
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
