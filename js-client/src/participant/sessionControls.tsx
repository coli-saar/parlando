import { type PlayerRole } from "../protocol.js";

/** Reports session identity and connection state and supplies the lifecycle's leave action. */
export function SessionTools({ connected, onLeave, role, sessionId }: {
  connected: boolean;
  onLeave: () => void;
  role: PlayerRole;
  sessionId: string;
}) {
  return (
    <section className="parlando-session-tools" aria-label="Session controls">
      <style>{`
        .parlando-session-tools{padding:1rem;border:1px solid var(--parlando-tools-border,currentColor);border-radius:var(--parlando-tools-radius,.75rem);background:var(--parlando-tools-background,transparent);color:var(--parlando-tools-color,inherit)}
        .parlando-session-tools-header{display:flex;flex-wrap:wrap;align-items:center;justify-content:space-between;gap:.75rem}
      `}</style>
      <header className="parlando-session-tools-header">
        <span><strong>Session {sessionId}</strong> · Player {role} · {connected ? "Connected" : "Reconnecting"}</span>
        <button className="parlando-leave-session" onClick={onLeave} type="button">Leave game</button>
      </header>
    </section>
  );
}
