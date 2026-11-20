/**
 * The startup shell accepts a same-origin administrator replay address and passes it to
 * ReplayApp. Only the current loader may publish a response or error. ReplayPlayer advances
 * through recorded game times and sends the selected role's read-only frames to the same
 * renderer used for live play.
 */

import { useEffect, useState, type ReactNode } from "react";
import { type PlayerRole } from "../protocol.js";
import { type GameSnapshot, type SessionReplay } from "./types.js";
import { errorMessage } from "../helpers.js";

/** Returns the path and query of a same-origin administrator endpoint. An absent,
 * malformed or out-of-scope address leaves the application in participant mode. */
export function replayRequestUrl(): string | null {
  if (typeof window === "undefined") return null;
  const value = new URLSearchParams(window.location.search).get("parlandoReplay");
  if (!value) return null;
  let url: URL;
  try {
    url = new URL(value, window.location.origin);
  } catch {
    return null;
  }
  if (url.origin !== window.location.origin || !url.pathname.startsWith("/api/admin/")) return null;
  return `${url.pathname}${url.search}`;
}

/** Loads an administrator replay without entering any participant lifecycle. */
export function ReplayApp<TObservation, TAction, TCompletion>({
  replayUrl,
  renderGame
}: {
  replayUrl: string;
  renderGame(snapshot: GameSnapshot<TObservation, TAction>): ReactNode;
}) {
  const [replay, setReplay] = useState<SessionReplay<TObservation, TAction, TCompletion> | null>(null);
  const [error, setReplayError] = useState("");
  useEffect(() => {
    let active = true;
    void fetch(replayUrl)
      .then(async (response) => {
        if (!response.ok) throw new Error(await response.text());
        return response.json() as Promise<SessionReplay<TObservation, TAction, TCompletion>>;
      })
      .then((value) => { if (active) setReplay(value); })
      .catch((caught) => { if (active) setReplayError(errorMessage(caught, "Could not load replay.")); });
    return () => { active = false; };
  }, [replayUrl]);
  if (error) return <section className="parlando-replay-error" role="alert">{error}</section>;
  if (!replay) return <section className="parlando-replay-loading" role="status">Loading replay…</section>;
  return <ReplayPlayer replay={replay} renderGame={renderGame} />;
}

/** Renders one recorded session through the same game surface used during live play. */
export function ReplayPlayer<TObservation, TAction, TCompletion = Record<string, unknown>>({
  replay,
  renderGame
}: {
  replay: SessionReplay<TObservation, TAction, TCompletion>;
  renderGame(snapshot: GameSnapshot<TObservation, TAction>): ReactNode;
}) {
  const [role, setRole] = useState<PlayerRole>("A");
  const [position, setPosition] = useState(0);
  const [playing, setPlaying] = useState(false);
  const frames = replay.roles[role] ?? [];
  const safePosition = Math.min(position, Math.max(0, frames.length - 1));
  const frame = frames[safePosition];
  useEffect(() => {
    if (!playing || !frame || safePosition >= frames.length - 1) {
      if (playing && safePosition >= frames.length - 1) setPlaying(false);
      return;
    }
    const delay = Math.max(100, frames[safePosition + 1].game_time_ms - frame.game_time_ms);
    const timer = window.setTimeout(() => setPosition((current) => current + 1), delay);
    return () => window.clearTimeout(timer);
  }, [frame, frames, playing, safePosition]);
  if (!frame) return <section className="parlando-replay-error" role="alert">This replay has no frames.</section>;
  const snapshot: GameSnapshot<TObservation, TAction> = {
    sessionId: replay.public_session_id,
    role,
    observation: frame.observation,
    transition: frame.transition,
    availableActions: frame.available_actions,
    interactionEnabled: false,
    act: () => undefined
  };
  return (
    <div className="parlando-replay">
      <style>{`.parlando-replay{min-height:100%;padding-bottom:6.5rem}.parlando-replay-conversation{position:fixed;z-index:999;top:1rem;right:1rem;display:grid;gap:.4rem;width:min(20rem,calc(100vw - 2rem));max-height:35vh;overflow:auto;margin:0;padding:.75rem;list-style:none;border:1px solid var(--parlando-replay-border,#68707c);border-radius:.8rem;background:var(--parlando-replay-background,rgba(20,24,31,.96));color:var(--parlando-replay-color,#fff)}.parlando-replay-message{display:grid;grid-template-columns:auto 1fr;gap:.5rem}.parlando-replay-message p{margin:0}.parlando-replay-controls{position:fixed;z-index:1000;left:50%;bottom:1rem;transform:translateX(-50%);display:grid;grid-template-columns:auto auto auto minmax(12rem,32rem) auto;gap:.6rem;align-items:center;width:min(54rem,calc(100vw - 2rem));padding:.8rem 1rem;border:1px solid var(--parlando-replay-border,#68707c);border-radius:.8rem;background:var(--parlando-replay-background,rgba(20,24,31,.96));color:var(--parlando-replay-color,#fff);box-shadow:0 .6rem 2rem rgba(0,0,0,.3)}.parlando-replay-controls button,.parlando-replay-controls select{font:inherit}.parlando-replay-controls input{width:100%}.parlando-replay-time{font-variant-numeric:tabular-nums;white-space:nowrap}@media(max-width:640px){.parlando-replay-controls{grid-template-columns:auto auto auto 1fr;}.parlando-replay-controls select{grid-column:1/-1}.parlando-replay-time{display:none}}`}</style>
      <div aria-disabled="true" className="parlando-replay-surface" inert>{renderGame(snapshot)}</div>
      {frame.conversation.length > 0 && <ol className="parlando-replay-conversation" aria-label="Conversation at this event">
        {frame.conversation.map((message) => <li className="parlando-replay-message" key={message.id}><strong>{message.sender}</strong><p>{message.text}</p></li>)}
      </ol>}
      <nav className="parlando-replay-controls" aria-label="Replay timeline">
        <button aria-label="Previous event" disabled={safePosition === 0} onClick={() => { setPlaying(false); setPosition(safePosition - 1); }} type="button">←</button>
        <button aria-label={playing ? "Pause replay" : "Play replay"} onClick={() => { if (safePosition >= frames.length - 1) setPosition(0); setPlaying((value) => !value); }} type="button">{playing ? "Pause" : "Play"}</button>
        <button aria-label="Next event" disabled={safePosition >= frames.length - 1} onClick={() => { setPlaying(false); setPosition(safePosition + 1); }} type="button">→</button>
        <input aria-label="Replay event" max={Math.max(0, frames.length - 1)} min={0} onChange={(event) => { setPlaying(false); setPosition(Number(event.target.value)); }} type="range" value={safePosition} />
        <span className="parlando-replay-time">{Math.round(frame.game_time_ms / 100) / 10}s · {safePosition + 1}/{frames.length}</span>
        <select aria-label="Replay viewpoint" onChange={(event) => setRole(event.target.value as PlayerRole)} value={role}>
          <option value="A">Player A view</option>
          <option value="B">Player B view</option>
        </select>
      </nav>
    </div>
  );
}
