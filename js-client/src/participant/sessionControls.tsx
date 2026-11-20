/**
 * SessionTools takes the runtime's conversation, input eligibility, and voice state, and
 * delegates sending, muting, and leaving through callbacks. It owns the chat draft across
 * pauses. Only an enabled submission with nonempty trimmed text calls onMessage and clears the
 * draft.
 */

import { useState } from "react";
import { type VoicePreflight, type VoiceStatus } from "../audio/types.js";
import { microphoneMuteButtonLabel } from "../helpers.js";
import { MicrophoneLevelMeter } from "../voiceComponents.js";
import { type PlayerMessage, type PlayerRole } from "../protocol.js";

/** Owns the local chat draft across pauses. Submission requires enabled interaction
 * and nonblank text. A successful handoff sends the trimmed text and clears the draft. */
export function SessionTools({
  connected,
  conversation,
  interactionEnabled,
  onLeave,
  onMessage,
  onMutedChange,
  role,
  sessionId,
  voiceEnabled,
  voicePreflight,
  voiceStatus
}: {
  connected: boolean;
  conversation: PlayerMessage[];
  interactionEnabled: boolean;
  onLeave: () => void;
  onMessage: (text: string) => void;
  onMutedChange: (muted: boolean) => Promise<void>;
  role: PlayerRole;
  sessionId: string;
  voiceEnabled: boolean;
  voicePreflight: VoicePreflight;
  voiceStatus: VoiceStatus;
}) {
  const [draft, setDraft] = useState("");
  const muted = voiceStatus.connected && !voiceStatus.microphoneEnabled;
  const messages = conversation.filter((message) => message.input === "text").slice(-6);
  return (
    <section className="parlando-session-tools" aria-label="Session controls">
      <style>{`.parlando-session-tools{display:grid;gap:.75rem;padding:1rem;border:1px solid var(--parlando-tools-border,currentColor);border-radius:var(--parlando-tools-radius,.75rem);background:var(--parlando-tools-background,transparent);color:var(--parlando-tools-color,inherit)}.parlando-session-tools-header,.parlando-session-tools-actions{display:flex;align-items:center;justify-content:space-between;gap:.75rem}.parlando-session-tools-conversation{display:grid;gap:.35rem;max-height:10rem;overflow:auto;margin:0;padding:0;list-style:none}.parlando-session-tools-message{display:grid;grid-template-columns:auto 1fr;gap:.5rem}.parlando-session-tools-message p{margin:0}.parlando-session-tools-form{display:flex;gap:.5rem}.parlando-session-tools-form input{min-width:0;flex:1}.parlando-session-tools-voice{display:grid;gap:.4rem}`}</style>
      <header className="parlando-session-tools-header">
        <span><strong>Session {sessionId}</strong> · Player {role} · {connected ? "Connected" : "Reconnecting"}</span>
        <button className="parlando-leave-session" onClick={onLeave} type="button">Leave game</button>
      </header>
      <ol className="parlando-session-tools-conversation" aria-label="Text conversation" data-testid="messages">
        {messages.length === 0 ? <li className="parlando-session-tools-empty">No messages yet.</li> : messages.map((message) => (
          <li className="parlando-session-tools-message" key={message.id}><strong>{message.sender}</strong><p>{message.text}</p></li>
        ))}
      </ol>
      <form className="parlando-session-tools-form" onSubmit={(event) => { event.preventDefault(); const text = draft.trim(); if (!interactionEnabled || !text) return; onMessage(text); setDraft(""); }}>
        <input aria-label="Message" disabled={!interactionEnabled} onChange={(event) => setDraft(event.target.value)} value={draft} />
        <button disabled={!interactionEnabled || !draft.trim()} type="submit">Send message</button>
      </form>
      {voiceEnabled && <div className={`parlando-session-tools-voice ${muted ? "muted" : "live"}`}>
        <div className="parlando-session-tools-actions">
          <strong>Microphone</strong>
          <button aria-pressed={muted} className={`microphone-mute-button ${muted ? "muted" : "live"}`} disabled={!voiceStatus.connected || voiceStatus.microphoneChanging} onClick={() => void onMutedChange(!muted)} type="button">{microphoneMuteButtonLabel(voiceStatus)}</button>
        </div>
        <MicrophoneLevelMeter active={voicePreflight.micProbeActive} label={voicePreflight.deviceLabel || "Level"} level={voicePreflight.micLevel} muted={muted} />
        <span data-testid="voice">{voiceStatus.message}</span>
        <span data-testid="remote-audio">{voiceStatus.remoteAudio ? "heard" : "silent"}</span>
        {voiceStatus.error && <p className="voice-error" role="alert">{voiceStatus.error}</p>}
      </div>}
    </section>
  );
}
