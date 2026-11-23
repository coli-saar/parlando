import { createContext, useContext, useLayoutEffect, useRef, useState, type FormEvent } from "react";
import { type VoicePreflight, type VoiceStatus } from "../audio/types.js";
import { type CommunicationMode, type PlayerMessage } from "../protocol.js";
import { MicrophoneLevelMeter, MicrophoneMuteButton } from "../voiceComponents.js";

/** Session-owned state shared by the standard and game-positioned widgets. */
type CommunicationState =
  | { mode: "typed"; conversation: PlayerMessage[]; enabled: boolean; send(text: string): boolean }
  | { mode: "speech"; preflight: VoicePreflight; status: VoiceStatus; setMuted(muted: boolean): void }
  | { mode: CommunicationMode; replay: PlayerMessage[] };

/** Provides the current channel and its actions to the widget inside the game renderer. */
export const CommunicationContext = createContext<CommunicationState | null>(null);

const styles = `
  .parlando-communication{display:grid;gap:.65rem;min-width:0;padding:.75rem;border:1px solid var(--parlando-communication-border,currentColor);border-radius:var(--parlando-communication-radius,.75rem);background:var(--parlando-communication-background,transparent);color:var(--parlando-communication-color,inherit)}
  .parlando-communication-conversation{display:grid;gap:.35rem;max-height:10rem;overflow:auto;margin:0;padding:0;list-style:none}
  .parlando-communication-message{display:grid;grid-template-columns:auto minmax(0,1fr);gap:.5rem}
  .parlando-communication-message p{margin:0;overflow-wrap:anywhere}
  .parlando-communication-form,.parlando-communication-controls{display:flex;flex-wrap:wrap;align-items:center;gap:.5rem}
  .parlando-communication-form input{flex:1;min-width:8rem}
  .parlando-communication :where(input,button){font:inherit}
  .parlando-communication .parlando-session-tools-voice{display:grid;gap:.25rem}
  .parlando-communication-error{margin:0}
`;

/**
 * Renders the current session's communication channel using the standard controls and styles.
 * Set ParticipantApp's communicationPlacement to "game" and place this component inside
 * renderGame to choose its position. During replay, it shows the recorded conversation.
 */
export function CommunicationWidget({ className }: { className?: string } = {}) {
  const state = useContext(CommunicationContext);
  if (!state) return null;
  return (
    <section aria-label="Communication" className={className ? `parlando-communication ${className}` : "parlando-communication"} data-mode={state.mode}>
      <style>{styles}</style>
      {"replay" in state ? (
        <Conversation label="Conversation at this event" messages={state.replay} />
      ) : state.mode === "typed" ? (
        <TypedCommunication conversation={state.conversation} enabled={state.enabled} send={state.send} />
      ) : (
        <SpeechCommunication preflight={state.preflight} status={state.status} setMuted={state.setMuted} />
      )}
    </section>
  );
}

/** Displays conversation messages in their supplied order, with a sender beside each message. */
function Conversation({ label, messages, announce = false }: { label: string; messages: PlayerMessage[]; announce?: boolean }) {
  const listRef = useRef<HTMLOListElement>(null);
  const followLatest = useRef(true);
  const latestId = messages.at(-1)?.id;

  // Follow incoming messages while the participant is at the bottom of the list.
  useLayoutEffect(() => {
    const list = listRef.current;
    if (followLatest.current && list) list.scrollTop = list.scrollHeight;
  }, [latestId]);

  /** Remembers whether the participant is reading the latest message or an older one. */
  function updateScrollPosition() {
    const list = listRef.current;
    if (list) followLatest.current = list.scrollHeight - list.scrollTop - list.clientHeight < 4;
  }

  return (
    <ol aria-label={label} aria-live={announce ? "polite" : "off"} aria-relevant="additions" className="parlando-communication-conversation parlando-session-tools-conversation" data-testid="messages" onScroll={updateScrollPosition} ref={listRef} tabIndex={0}>
      {messages.length === 0 ? (
        <li className="parlando-communication-empty parlando-session-tools-empty">No messages yet.</li>
      ) : messages.map((message) => (
        <li className="parlando-communication-message parlando-session-tools-message" key={message.id}>
          <strong>{message.sender}</strong><p>{message.text}</p>
        </li>
      ))}
    </ol>
  );
}

/** Keeps the local draft across connection changes and submits only nonempty text during play. */
function TypedCommunication({ conversation, enabled, send }: {
  conversation: PlayerMessage[];
  enabled: boolean;
  send(text: string): boolean;
}) {
  const [draft, setDraft] = useState("");

  /** Normalizes the draft and clears it after handing it to the game connection. */
  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const text = draft.trim();
    if (!enabled || !text) return;
    if (send(text)) setDraft("");
  }

  return (
    <>
      <Conversation announce label="Text conversation" messages={conversation.filter((message) => message.input === "text")} />
      <form className="parlando-communication-form parlando-session-tools-form" onSubmit={submit}>
        <input aria-label="Message" disabled={!enabled} onChange={(event) => setDraft(event.target.value)} value={draft} />
        <button disabled={!enabled || !draft.trim()} type="submit">Send message</button>
      </form>
    </>
  );
}

/** Shows the current microphone, connection feedback, and transport-owned mute state. */
function SpeechCommunication({ preflight, status, setMuted }: {
  preflight: VoicePreflight;
  status: VoiceStatus;
  setMuted(muted: boolean): void;
}) {
  const muted = status.connected && !status.microphoneEnabled;
  return (
    <div className={`parlando-session-tools-voice ${muted ? "muted" : "live"}`}>
      <div className="parlando-communication-controls parlando-session-tools-actions">
        <MicrophoneMuteButton enabled onMutedChange={setMuted} status={status} />
      </div>
      <MicrophoneLevelMeter active={preflight.micProbeActive} label={preflight.deviceLabel || "Microphone level"} level={preflight.micLevel} muted={muted} />
      <span className="parlando-communication-status" data-testid="voice">{status.message}</span>
      {status.error && <p className="parlando-communication-error voice-error" role="alert">{status.error}</p>}
    </div>
  );
}
