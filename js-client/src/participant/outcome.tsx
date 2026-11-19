import { useEffect, useRef, useState, type ReactNode } from "react";
import { type VoiceStatus } from "../audio/types.js";
import { microphoneMuteButtonLabel } from "../helpers.js";
import { type ExperimentInfo, type ParticipantOutcome, type RecruitmentHandoff } from "../protocol.js";

/** Standard terminal surface for provider-neutral outcomes and recruitment handoff. */
export function SessionOutcomePanel({
  outcome,
  reason,
  handoff,
  recruitment,
  completionContent,
  farewellContent
}: {
  outcome: ParticipantOutcome | null;
  reason: string | null;
  handoff: RecruitmentHandoff | null;
  recruitment?: ExperimentInfo["recruitment"];
  completionContent?: ReactNode;
  farewellContent?: ReactNode;
}) {
  const heading = outcome === "completed" ? "Session complete" : "Session ended";
  return (
    <section className={`parlando-session-outcome outcome-${outcome ?? "unknown"}`}>
      <h1>{heading}</h1>
      <p>{outcomeText(outcome, reason)}</p>
      {completionContent && (
        <div className="parlando-game-completion">{completionContent}</div>
      )}
      {farewellContent}
      {handoff && (
        <ProlificHandoff handoff={handoff} />
      )}
    </section>
  );
}

/** Standard terminal notice for the bounded participant-to-participant voice period. */
export function FarewellVoiceNotice({
  deadlineAt,
  onEnd,
  onMutedChange,
  status
}: {
  deadlineAt: string;
  onEnd: () => void;
  onMutedChange: (muted: boolean) => void;
  status: VoiceStatus;
}) {
  const [now, setNow] = useState(Date.now());
  const endedRef = useRef(false);
  const remainingSeconds = Math.max(0, Math.ceil((Date.parse(deadlineAt) - now) / 1_000));
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, []);
  useEffect(() => {
    if (remainingSeconds > 0 || endedRef.current) return;
    endedRef.current = true;
    onEnd();
  }, [onEnd, remainingSeconds]);
  const muted = status.connected && !status.microphoneEnabled;
  return (
    <section className="parlando-farewell-voice">
      <strong>Voice chat remains open</strong>
      {remainingSeconds > 0 && status.connected ? (
        <>
          <span>{formatCountdown(remainingSeconds)} remaining</span>
          <span>You can talk with your partner until the timer reaches zero. This conversation is not transcribed or stored.</span>
          <div className="parlando-farewell-actions">
            <button
              aria-pressed={muted}
              className={`microphone-mute-button ${muted ? "muted" : "live"}`}
              disabled={status.microphoneChanging}
              onClick={() => onMutedChange(!muted)}
              type="button"
            >
              {microphoneMuteButtonLabel(status)}
            </button>
            <button onClick={onEnd} type="button">End voice chat</button>
          </div>
        </>
      ) : (
        <span>Voice chat has closed.</span>
      )}
    </section>
  );
}

/** Formats one non-negative countdown as minutes and two-digit seconds. */
function formatCountdown(seconds: number): string {
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}

/** Premade Prolific completion-code widget appended to a game's terminal content. */
export function ProlificHandoff({ handoff }: { handoff: RecruitmentHandoff }) {
  const [copyState, setCopyState] = useState<"idle" | "copied" | "failed">("idle");

  /** Copies the completion code and exposes the result to sighted and screen-reader users. */
  async function copyCode(): Promise<void> {
    try {
      if (!navigator.clipboard) throw new Error("Clipboard API unavailable");
      await navigator.clipboard.writeText(handoff.code);
      setCopyState("copied");
    } catch {
      setCopyState("failed");
    }
  }

  return (
    <div className="parlando-recruitment-handoff">
      <p>Use this completion code in Prolific:</p>
      <div className="parlando-completion-code-row">
        <strong className="parlando-completion-code">{handoff.code}</strong>
        <button
          aria-label="Copy Prolific completion code"
          className="parlando-copy-completion-code"
          onClick={copyCode}
          type="button"
        >
          {copyState === "copied" ? "Copied" : "Copy"}
        </button>
      </div>
      <span aria-live="polite" className="parlando-copy-status">
        {copyState === "copied" && "Copied to clipboard."}
        {copyState === "failed" && "Could not copy automatically. Select the code and copy it manually."}
      </span>
      <a href={handoff.url}>Return to Prolific</a>
    </div>
  );
}

/** Converts stable outcomes into concise participant-facing explanations. */
function outcomeText(outcome: ParticipantOutcome | null, reason: string | null): string {
  switch (outcome) {
    case "completed": return "Thank you. Your responses have been recorded.";
    case "left_waiting_room": return "You left before a playable game began. Use the Game did not start completion path below to return to Prolific.";
    case "left_game": return "You left after the game started. Your responses up to that point have been recorded.";
    case "connection_lost": return "The session ended because your connection did not return before the reconnect deadline.";
    case "partner_left": return "Your partner left or could not reconnect, so the session cannot continue.";
    case "partner_unavailable": return "No partner became available before the waiting period ended.";
    case "idle_limit_reached": return "The session ended because neither participant produced meaningful activity before the inactivity deadline.";
    case "technical_failure": return "A technical problem prevented the session from continuing.";
    case "lifetime_limit_reached": return "The session reached its absolute lifetime limit and could not continue.";
    default: return reason ? `The session ended (${reason}).` : "The session can no longer continue.";
  }
}
