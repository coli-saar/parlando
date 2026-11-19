import { type ReactNode } from "react";
import { type VoiceStatus } from "../audio/types.js";
import { type Presence } from "../protocol.js";

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
      <div className="lobby-heading">
        <p className="platform-label">{platformLabel(institution)}</p>
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

/** Displays connection and service readiness while participants wait. */
export function ReadinessBoard({
  connected,
  enabled,
  presence,
  status
}: {
  connected: boolean;
  enabled: boolean;
  presence: Presence;
  status: VoiceStatus;
}) {
  const aConnected = Boolean(presence.A?.connected);
  const bConnected = Boolean(presence.B?.connected);
  return (
    <div className="seat-grid">
      <div className={aConnected ? "seat-ready" : ""}>
        <strong>Player A</strong>
        <span>{aConnected ? "Connected" : "Waiting"}</span>
      </div>
      <div className={bConnected ? "seat-ready" : ""}>
        <strong>Player B</strong>
        <span>{bConnected ? "Connected" : "Waiting"}</span>
      </div>
      {enabled && (
        <div className={status.transcriptionReady ? "seat-ready" : ""}>
          <strong>Transcription Service</strong>
          <span>{status.transcriptionReady ? "Ready" : connected ? status.transcriptionMessage : "Not started"}</span>
        </div>
      )}
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
