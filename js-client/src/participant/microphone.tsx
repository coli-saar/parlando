/**
 * The microphone preparation UI adapts browser device choices, preflight readings, and server
 * voice updates for the participant runtime. It calls the voice hook's selection and
 * preparation callbacks and disables selection during preparation. Its default controller
 * pairs MicrophoneSource with ParlandoAudioSink.
 */

import { AudioSessionController } from "../audio/audioSessionController.js";
import { ParlandoAudioSink } from "../audio/parlandoAudioSink.js";
import { MicrophoneSource } from "../audio/microphoneSource.js";
import { initialVoicePreflight, type VoicePreflight, type VoiceStatus } from "../audio/types.js";
import { MicrophoneLevelMeter } from "../voiceComponents.js";
import { type ExperimentInfo } from "../protocol.js";

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
  allowDeviceChange = true,
  onPrepareVoice,
  selectedAudioInputId,
  voicePreflight = initialVoicePreflight
}: {
  audioInputs: MediaDeviceInfo[];
  enabled: boolean;
  allowDeviceChange?: boolean;
  onPrepareVoice: (deviceId?: string) => boolean | void | Promise<boolean | void>;
  selectedAudioInputId: string;
  voicePreflight?: VoicePreflight;
}) {
  const availableMicrophones = selectableAudioInputs(audioInputs);
  return (
    <>
      {(voicePreflight.ready || voicePreflight.preparing) && (
        <select
          aria-label="Microphone input"
          disabled={!allowDeviceChange || voicePreflight.preparing || availableMicrophones.length < 2}
          onChange={(event) => {
            const deviceId = event.target.value;
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
      <MicrophoneLevelMeter active={voicePreflight.micProbeActive} label="Level" level={voicePreflight.micLevel} />
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
  return config?.communication.mode === "speech";
}

/** @internal Returns concrete microphone inputs, excluding the synthetic default-device alias. */
export function selectableAudioInputs(audioInputs: MediaDeviceInfo[]): MediaDeviceInfo[] {
  return audioInputs.filter((device) => device.deviceId !== "default");
}

/** @internal Removes browser-added USB identifiers from a microphone name. */
export function participantMicrophoneLabel(label: string): string {
  return label.replace(/\s*[([][0-9a-f]{4}:[0-9a-f]{4}[)\]]\s*$/i, "").trim() || "Microphone";
}

/** @internal Normalizes a wire voice-status update. */
export function voiceStatusUpdate(voice: { audioReady?: boolean; transcriptionReady?: boolean; transcriptionStatus?: string } | undefined): Partial<VoiceStatus> {
  const update: Partial<VoiceStatus> = {};
  if (voice?.transcriptionStatus) update.transcriptionMessage = voice.transcriptionStatus;
  if (typeof voice?.transcriptionReady === "boolean") update.transcriptionReady = voice.transcriptionReady;
  return update;
}
