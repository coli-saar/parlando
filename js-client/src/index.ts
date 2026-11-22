/**
 * Game clients import the framework-independent protocol API here. ParticipantClient handles
 * requests, reduceParticipantState applies server snapshots, and the exported types describe
 * the participant and voice state visible to callers. The React entry point supplies the UI.
 */

export {
  ParticipantClient,
  reduceParticipantState,
  type AudioSessionPlan,
  type ConsentItem,
  type CommunicationMode,
  type ExperimentInfo,
  type ParticipantState,
  type ParticipantResult,
  type ParticipantPauseReason,
  type GameSessionPlan,
  type MessageInput,
  type PlayerMessage,
  type PlayerPresence,
  type PlayerRole,
  type ParticipantClientOptions,
  type Presence
} from "./protocol.js";

export type { VoicePreflight, VoiceStatus } from "./audio/types.js";
