/**
 * The participant runtime supplies startup text, connection state, player presence, and local
 * voice status to the lobby shell and readiness board. Presence normalization supplies a
 * consistent shape for each seat. The board displays readiness while the server decides when
 * the session starts.
 */

import { type ReactNode } from "react";
import { type Presence, type PlayerRole } from "../protocol.js";

/** Displays the common experiment heading, participant controls, and error feedback. */
export function StartupShell({
  body,
  children,
  error,
  gameName,
  heading,
  institution
}: {
  body?: string;
  children?: ReactNode;
  error: string;
  gameName?: string;
  heading?: string;
  institution?: string | null;
}) {
  return (
    <section className="lobby-panel">
      <style>{`
        @layer parlando {
          .lobby-panel .voice-preflight, .lobby-panel .consent-list {
            display: grid;
            gap: .75rem;
            margin-block: 1rem;
          }
          .lobby-panel .voice-preflight {
            padding: 1rem;
            border: 1px solid color-mix(in srgb, currentColor 35%, transparent);
            border-radius: .75rem;
          }
          .lobby-panel .voice-preflight > div:first-child,
          .lobby-panel .consent-row > span { display: grid; gap: .2rem; }
          .lobby-panel .voice-preflight > button { justify-self: start; }
          .lobby-panel .voice-preflight select {
            box-sizing: border-box;
            min-width: 0;
            max-width: 100%;
            padding: .4rem .6rem;
            border: 1px solid currentColor;
            border-radius: .4rem;
            font: inherit;
            color: inherit;
            background-color: transparent;
          }
          .lobby-panel .consent-row { display: flex; align-items: baseline; gap: .5rem; }
        }
      `}</style>
      <div className="lobby-heading">
        <p className="platform-label">
          <img
            src={new URL("./parlando-mark.svg", import.meta.url).href}
            width="20" height="20" alt=""
            style={{ verticalAlign: "middle", marginInlineEnd: "0.4em", display: "inline-block" }}
          />
          {platformLabel(institution)}
        </p>
        {gameName && <p className="game-name">{gameName}</p>}
        {heading && <h1>{heading}</h1>}
      </div>
      {body && (
        <div className="lobby-copy">
          <p>{body}</p>
        </div>
      )}
      {children}
      {error && <p className="online-error">{error}</p>}
    </section>
  );
}

/** Shows both seats from the waiting guest’s perspective using server-reported presence. */
export function ReadinessBoard({ presence, role }: { presence: Presence; role?: PlayerRole }) {
  const first = role ?? "A";
  const seats: PlayerRole[] = [first, first === "A" ? "B" : "A"];
  return (
    <div className="seat-grid">
      {seats.map(seat => (
        <div key={seat} className={presence[seat]?.connected ? "seat-ready" : ""}>
          <strong>{role ? (seat === role ? "You" : "Your partner") : "Player " + seat}</strong>{" "}
          <span>{presence[seat]?.connected ? "Connected" : "Waiting"}</span>
        </div>
      ))}
    </div>
  );
}

/** @internal Formats the stable platform label with an optional operating institution. */
export function platformLabel(institution?: string | null): string {
  const name = institution?.trim();
  return name ? `Parlando · ${name}` : "Parlando";
}

/** @internal Normalizes wire presence into the public two-role shape. */
export function normalizePresence(presence: Record<string, unknown> | undefined): Presence {
  return {
    A: normalizeSeat(presence?.A),
    B: normalizeSeat(presence?.B)
  };
}

/** Converts a single wire seat to its public presence representation. */
function normalizeSeat(value: unknown): Presence["A"] {
  if (!value || typeof value !== "object") return undefined;
  const record = value as Record<string, unknown>;
  return {
    connected: Boolean(record.connected),
    audioReady: typeof record.audioReady === "boolean" ? record.audioReady : undefined
  };
}
