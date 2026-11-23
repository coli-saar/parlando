/**
 * Microphone and transcription indicators translate level readings and VoiceStatus into
 * participant-visible feedback. Preparation and session controls supply the readings. The
 * audio controller owns device access and connection changes.
 */

import { useMemo } from "react";
import type { VoiceStatus } from "./audio/types.js";
import { initialVoiceStatus } from "./audio/types.js";
import { microphoneMuteButtonLabel, transcriptionProgressForStatus } from "./helpers.js";

/** Renders a normalized local microphone level, with a colorless fill while transport-muted. */
export function MicrophoneLevelMeter({
  active,
  label,
  level,
  muted = false
}: {
  active: boolean;
  label: string;
  level: number;
  muted?: boolean;
}) {
  const normalizedLevel = Number.isFinite(level) ? Math.max(0, Math.min(1, level)) : 0;
  return (
    <div className={`mic-meter ${muted ? "muted" : "live"}`}>
      <style>{`
        @layer parlando {
          .mic-meter { display: grid; gap: .25rem; min-width: 0; }
          .mic-meter-track {
            display: block;
            height: .5rem;
            overflow: hidden;
            border: 1px solid currentColor;
            border-radius: 1rem;
          }
          .mic-meter-track > span {
            display: block;
            height: 100%;
            background: var(--parlando-communication-meter-color, currentColor);
            transform-origin: left;
          }
        }
      `}</style>
      <span>{label}</span>
      <div className="mic-meter-track" aria-hidden="true">
        <span style={{ transform: `scaleX(${active ? normalizedLevel : 0})` }} />
      </div>
    </div>
  );
}

/** Renders the shared hosted-transcription readiness progression. */
export function TranscriptionProgress({
  connected = true,
  status = initialVoiceStatus
}: {
  connected?: boolean;
  status?: VoiceStatus;
}) {
  const progress = useMemo(
    () => transcriptionProgressForStatus(status.transcriptionMessage, status.transcriptionReady),
    [status.transcriptionMessage, status.transcriptionReady]
  );
  const transportStarted = connected && (status.connecting || status.connected);
  const heading = !connected
    ? "Waiting for game connection"
    : !transportStarted
      ? "Preparing voice connection"
      : status.transcriptionReady
        ? "Transcription ready"
        : "Waiting for transcription service";
  const detail = !connected
    ? "Transcription has not started."
    : !transportStarted
      ? "Transcription starts after the voice transport connects."
      : status.transcriptionMessage;
  return (
    <div className="transcription-progress" aria-label="Transcription service progress">
      <div>
        <strong>{heading}</strong>
        <span>{detail}</span>
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

/** Renders the participant control for muting an already connected microphone. */
export function MicrophoneMuteButton({
  enabled,
  onMutedChange,
  status = initialVoiceStatus
}: {
  enabled: boolean;
  onMutedChange: (muted: boolean) => void;
  status?: VoiceStatus;
}) {
  const muted = status.connected && !status.microphoneEnabled;
  return (
    <button
      aria-pressed={muted}
      className={`microphone-mute-button ${muted ? "muted" : "live"}`}
      disabled={!enabled || !status.connected || status.microphoneChanging}
      onClick={() => onMutedChange(!muted)}
      type="button"
    >
      {microphoneMuteButtonLabel(status)}
    </button>
  );
}
