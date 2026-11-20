/**
 * The participant runtime supplies server timestamps for waiting, reconnect, and idle
 * deadlines. These notices update the displayed time locally, clamp elapsed countdowns to
 * zero, and release their timers on unmount. The server determines the resulting session
 * outcome.
 */

import { useEffect, useState } from "react";

/** Shared fixed-deadline guidance for an unmatched waiting session. */
export function WaitingRoomNotice({
  deadlineAt,
  prolific,
  startedAt
}: {
  deadlineAt: string;
  prolific: boolean;
  startedAt: string;
}) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 250);
    return () => window.clearInterval(timer);
  }, []);
  const deadline = Date.parse(deadlineAt);
  const started = Date.parse(startedAt);
  const remainingSeconds = Math.max(0, Math.ceil((deadline - now) / 1000));
  const maximumMinutes = Math.max(1, Math.ceil((deadline - started) / 60_000));
  const remainingMinutes = Math.floor(remainingSeconds / 60);
  const seconds = remainingSeconds % 60;
  const terminalGuidance = prolific
    ? "If no partner joins, or you leave now, the game will not start. You will return to Prolific and be asked to return your submission. The researcher can issue a partial payment for the time you waited."
    : "If no partner joins, or you leave now, this session will end without starting the game.";
  return (
    <section aria-live="polite" className="parlando-waiting-room" role="status">
      <strong>Waiting for your partner</strong>
      <span className="parlando-waiting-countdown">
        {remainingMinutes}:{String(seconds).padStart(2, "0")} remaining
      </span>
      <span>The maximum wait is {maximumMinutes} minute{maximumMinutes === 1 ? "" : "s"}. Keep this tab open.</span>
      <span>{terminalGuidance}</span>
    </section>
  );
}

/** Stylable, self-updating notice shown while a required partner may reconnect. */
export function PartnerReconnectNotice({ deadlineAt }: { deadlineAt: string }) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 250);
    return () => window.clearInterval(timer);
  }, []);
  const seconds = Math.max(0, Math.ceil((Date.parse(deadlineAt) - now) / 1000));
  return (
    <section aria-live="polite" className="parlando-partner-reconnect" role="status">
      <strong>Your partner lost their connection</strong>
      <span>The game is paused while they reconnect. It will end in {seconds} second{seconds === 1 ? "" : "s"}.</span>
    </section>
  );
}

/** Warns both participants when the shared meaningful-activity deadline is close. */
export function IdleDeadlineNotice({ deadlineAt }: { deadlineAt: string }) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, []);
  const seconds = Math.max(0, Math.ceil((Date.parse(deadlineAt) - now) / 1_000));
  if (seconds > 60) return null;
  return (
    <section aria-live="polite" className="parlando-idle-warning" role="status">
      <strong>Inactivity limit approaching</strong>
      <span>Send a message or make a game action within {seconds} second{seconds === 1 ? "" : "s"} to keep the session active.</span>
    </section>
  );
}
