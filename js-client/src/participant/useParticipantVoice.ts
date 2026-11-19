import { useCallback, useEffect, useRef, useState } from "react";
import { type LiveSession } from "../participantConnection.js";
import { AudioSessionController } from "../audio/audioSessionController.js";
import { initialVoicePreflight, initialVoiceStatus, type AudioSessionContext, type VoicePreflight, type VoiceStatus } from "../audio/types.js";
import type { ParticipantClient, ParticipantOutcome } from "../protocol.js";
import { errorMessage } from "../helpers.js";

/** Binds voice preparation and reconnection to the current seat. Each preparation
 * supersedes earlier permission requests. Cancellation makes a pending request
 * resolve false without publishing its error. Passive rescans keep the most recent
 * device list and preserve the last successful list when enumeration fails. */
export function useParticipantVoice<TObservation, TAction, TCompletion>(
  audioController: AudioSessionController,
  apiClient: ParticipantClient,
  session: LiveSession<TObservation, TAction, TCompletion> | null,
  enabled: boolean,
  setError: (message: string) => void,
) {
  const [audioInputs, setAudioInputs] = useState<MediaDeviceInfo[]>([]);
  const [selectedAudioInputId, setSelectedAudioInputId] = useState("");
  const [status, setVoiceStatus] = useState<VoiceStatus>(initialVoiceStatus);
  const [voicePreflight, setVoicePreflight] = useState<VoicePreflight>(initialVoicePreflight);
  const [voiceReconnectGeneration, setVoiceReconnectGeneration] = useState(0);

  const voicePreparationGenerationRef = useRef(0);

  const deviceListGeneration = useRef(0);

  /** Enumerates available microphone inputs for selection before entering the room. */
  const refreshAudioInputs = useCallback(async () => {
    if (!navigator.mediaDevices?.enumerateDevices) return [];
    const generation = ++deviceListGeneration.current;
    const devices = await navigator.mediaDevices.enumerateDevices();
    const inputs = devices.filter((device) => device.kind === "audioinput");
    if (generation === deviceListGeneration.current) setAudioInputs(inputs);
    return inputs;
  }, []);

  /** Invalidates pending permissions or device enumeration when the seat is released. */
  const cancelPreparation = useCallback(() => { voicePreparationGenerationRef.current += 1; }, []);

  /** Prepares the latest requested microphone and ignores superseded permission results. */
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
  }, [audioController, audioInputs, refreshAudioInputs, enabled, setError]);

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
  }, [audioController, currentAudioContext, setError]);

  /** Applies a participant-requested mute state while retaining the live voice transport. */
  const setMicrophoneMuted = useCallback(async (muted: boolean) => {
    const context = currentAudioContext();
    if (!context) return;
    await audioController.setMicrophoneMuted(muted, context);
  }, [audioController, currentAudioContext]);

  useEffect(() => {
    // A passive device scan may be denied before microphone permission is granted.
    const onDeviceChange = () => void refreshAudioInputs().catch(() => undefined);
    onDeviceChange();
    const devices = navigator.mediaDevices;
    devices?.addEventListener?.("devicechange", onDeviceChange);
    return () => {
      deviceListGeneration.current += 1;
      devices?.removeEventListener?.("devicechange", onDeviceChange);
    };
  }, [refreshAudioInputs]);

  useEffect(() => audioController.subscribe((snapshot) => {
    setVoiceStatus(snapshot.voiceStatus);
    setVoicePreflight(snapshot.voicePreflight);
  }), [audioController]);

  useEffect(() => {
    if (status.connected) setVoiceReconnectGeneration(0);
  }, [status.connected]);

  useEffect(() => {
    if (session?.synchronization !== "connected" || session.participantState.state === "ended" || !enabled || !voicePreflight.ready || status.connected || status.connecting) return;
    const delays = [1_000, 2_000, 5_000, 10_000];
    const timer = window.setTimeout(
      () => void connectVoice(),
      delays[Math.min(voiceReconnectGeneration, delays.length - 1)]
    );
    return () => window.clearTimeout(timer);
  }, [connectVoice, session?.synchronization, enabled, voicePreflight.ready, status.connected, status.connecting, voiceReconnectGeneration]);

  return {
    audioInputs, selectedAudioInputId, status, voicePreflight,
    cancelPreparation, prepareVoice, setMicrophoneMuted
  };
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

/** Resolves the browser label for a selected or default input. */
function selectedAudioInputLabel(audioInputs: MediaDeviceInfo[], selectedAudioInputId: string): string {
  if (!selectedAudioInputId) return "Default microphone";
  return audioInputs.find((device) => device.deviceId === selectedAudioInputId)?.label || "Selected microphone";
}
