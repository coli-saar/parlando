import type { ConsentItem, PublicConfigResponse } from "./protocol.js";
import type { VoiceStatus } from "./audio/types.js";

export interface PresenceState {
  A?: { participantSessionId?: string; connected?: boolean; audioReady?: boolean };
  B?: { participantSessionId?: string; connected?: boolean; audioReady?: boolean };
}

export function requiredConsentsAccepted(
  config: Pick<PublicConfigResponse, "consents"> | null,
  decisions: Record<string, boolean>
): boolean {
  if (!config) return true;
  return config.consents.every((consent: ConsentItem) => !consent.required || decisions[consent.id] === true);
}

export function bothPlayersConnected(presence: PresenceState): boolean {
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
  const completedIndex = ready ? stages.length - 1 : Math.max(0, matchedIndex);
  return {
    value: ready ? 1 : (completedIndex + 1) / stages.length,
    steps: stages.map((stage, index) => ({ label: stage.label, done: index <= completedIndex }))
  };
}

export function voiceButtonLabel(status: VoiceStatus): string {
  if (status.connected) return status.microphoneEnabled ? "Mute mic" : "Unmute mic";
  return status.connecting ? "Connecting" : "Join voice";
}
