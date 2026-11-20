/**
 * ParticipantApp selects the live participant runtime or the administrator replay from the
 * page URL. Both paths use the game's renderGame callback. Each accepted replay URL gets a
 * fresh player, while live play uses ParticipantClient and a session audio controller.
 */

import { useMemo } from "react";
import { ParticipantClient } from "./protocol.js";
import { type ParticipantAppProps } from "./participant/types.js";
import { replayRequestUrl, ReplayApp } from "./participant/replay.js";
import { createDefaultAudioController } from "./participant/microphone.js";
import { ParticipantAppRuntime } from "./participant/runtime.js";

export { type GameSnapshot, type GameTransition, type ParticipantAppProps, type ReplayFrame, type SessionReplay } from "./participant/types.js";
export { ReplayPlayer } from "./participant/replay.js";
export { PartnerReconnectNotice } from "./participant/deadlines.js";
export { ProlificHandoff } from "./participant/outcome.js";

/** Renders the participant lifecycle or the requested administrator replay.
 * Each replay URL owns its loading, playback and error state. Switching URLs
 * releases the previous view and ignores any response still in flight. */
export function ParticipantApp<
  TObservation,
  TAction = unknown,
  TCompletion = Record<string, unknown>
>({ renderGame, renderCompletion, baseUrl }: ParticipantAppProps<TObservation, TAction, TCompletion>) {
  const apiClient = useMemo(() => new ParticipantClient({ baseUrl }), [baseUrl]);
  const replayUrl = replayRequestUrl();
  if (replayUrl) {
    return <ReplayApp key={replayUrl} replayUrl={replayUrl} renderGame={renderGame} />;
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
