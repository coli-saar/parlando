import { microphoneMuteButtonLabel } from "./helpers.js";
import type { VoiceStatus } from "./audio/types.js";
import { initialVoiceStatus } from "./audio/types.js";

export { MicrophoneLevelMeter, TranscriptionProgress } from "./voiceComponents.js";

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

export {
  ParticipantApp,
  type GameSession,
  type ParticipantAppProps
} from "./startup.js";
