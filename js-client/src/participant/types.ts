/**
 * These types define the boundary between framework-owned session state and a game's React
 * renderer. Live play supplies one role's GameSnapshot and an action callback. Replay adapts
 * recorded frames for the selected role to the same renderer with interaction disabled.
 */

import { type ReactNode } from "react";
import { type CommunicationMode, type PlayerMessage, type PlayerRole } from "../protocol.js";

/** A role-specific view of the live game and its available interactions. */
export interface GameSnapshot<TObservation, TAction> {
  sessionId: string;
  role: PlayerRole;
  /** Server-reported admission source for this seat, or null during replay or when unavailable. */
  recruitmentSource: "direct" | "local_preview" | "prolific" | null;
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

/** Places the communication widget in the standard layout or inside the game renderer. */
export type CommunicationPlacement = "default" | "game";

/** Supplies game rendering and optional experiment server location. */
export interface ParticipantAppProps<TObservation, TAction, TCompletion = Record<string, unknown>> {
  renderGame(snapshot: GameSnapshot<TObservation, TAction>): ReactNode;
  /** Renders game-specific public success content inside the standard terminal shell. */
  renderCompletion?(completion: TCompletion): ReactNode;
  /** Choose "game" to render CommunicationWidget inside renderGame. Defaults to "default". */
  communicationPlacement?: CommunicationPlacement;
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
  communication: { mode: CommunicationMode };
  compatibility: { state: string; action: string; rules: string };
  roles: Record<PlayerRole, ReplayFrame<TObservation, TAction, TCompletion>[]>;
}
