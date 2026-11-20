/**
 * The participant runtime and voice controls use these pure decisions to interpret consent,
 * intake, presence, and voice status. Each caller supplies the current protocol or audio
 * snapshot and uses the returned eligibility, progress, or label when rendering.
 */

import type { ConsentItem, ExperimentInfo, Presence } from "./protocol.js";
import type { VoiceStatus } from "./audio/types.js";

export function requiredConsentsAccepted(
  config: Pick<ExperimentInfo, "consents"> | null,
  decisions: Record<string, boolean>
): boolean {
  if (!config) return true;
  return config.consents.every((consent: ConsentItem) => !consent.required || decisions[consent.id] === true);
}

/** Reports whether the experiment lifecycle currently permits participant intake. */
export function experimentAllowsIntake(
  status: ExperimentInfo["status"] | null | undefined
): boolean {
  return status === "testing" || status === "active";
}

export function bothPlayersConnected(presence: Presence): boolean {
  return Boolean(presence.A?.connected && presence.B?.connected);
}

export function transcriptionProgressForStatus(
  status: string,
  ready: boolean
): { value: number; steps: { label: string; done: boolean }[] } {
  const stages = [
    { label: "Starting", statuses: ["ASR worker starting", "Waiting for transcription service", "Waiting for ASR worker"] },
    { label: "Connected", statuses: ["ASR worker connected"] },
    { label: "Mic track", statuses: ["ASR has mic track"] },
    { label: "Listening", statuses: ["ASR listening"] },
    {
      label: "Audio",
      statuses: ["Audio reaching ASR", "Audio level detected", "Silent audio reaching ASR", "ASR transcribing"]
    }
  ];
  const matchedIndex = stages.findIndex((stage) => stage.statuses.includes(status));
  const completedIndex = ready ? stages.length - 1 : matchedIndex;
  return {
    value: ready ? 1 : matchedIndex < 0 ? 0 : (matchedIndex + 1) / stages.length,
    steps: stages.map((stage, index) => ({ label: stage.label, done: index <= completedIndex }))
  };
}

/** Returns the action label for the explicit microphone mute control. */
export function microphoneMuteButtonLabel(status: VoiceStatus): string {
  if (!status.connected) return status.connecting ? "Connecting" : "Microphone unavailable";
  if (status.microphoneChanging) return status.microphoneEnabled ? "Muting…" : "Unmuting…";
  return status.microphoneEnabled ? "Mute mic" : "Unmute mic";
}

/** Uses the error message when a browser or transport operation provides one. */
export function errorMessage(caught: unknown, fallback: string): string {
  return caught instanceof Error ? caught.message : fallback;
}
