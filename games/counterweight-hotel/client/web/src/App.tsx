import { useEffect, useState, type FormEvent } from "react";
import { MicrophoneMuteButton, ParticipantApp } from "@coli-saar/parlando-client/react";
import type { PlayerMessage, VoiceStatus } from "@coli-saar/parlando-client";
import { GameView, type Snapshot } from "./GameView";
import type { Action, Completion, Observation } from "./game";

import "./ending.css";

type RegistrySession = Omit<Snapshot, "act"> & {
  sendAction(action: Action): void;
  connected: boolean;
  conversation: PlayerMessage[];
  sendMessage(message: string): void;
  voiceEnabled: boolean;
  voiceStatus: VoiceStatus;
  setMicrophoneMuted(muted: boolean): Promise<void>;
  leave(): void;
};

/** Provides the game surface and ending inside Parlando's participant lifecycle. */
export function App() {
  return (
    <ParticipantApp<Observation, Action, Completion>
      renderGame={renderHotel}
      renderCompletion={(completion) => <HotelEnding completion={completion} />}
    />
  );
}

/** Frames the shared refuge after the server confirms that both guests have crossed. */
export function HotelEnding({ completion }: { completion: Completion }) {
  useEffect(() => {
    const bells = new Audio(new URL("./art/refuge-bells.wav", import.meta.url).href);
    bells.volume = 0.52;
    void bells.play().catch(() => {});
    return () => bells.pause();
  }, []);
  return (
    <section className="hotel-ending" aria-labelledby="hotel-ending-title">
      <div className="hotel-ending-sky" aria-hidden="true" />
      <div className="hotel-ending-copy">
        <p className="hotel-ending-place">Counterweight Hotel · Mountain refuge</p>
        <h2 id="hotel-ending-title">You escaped together</h2>
        <p>The refuge bells ring out across the ravine. Both guests are home.</p>
        {completion.route === "rescue_supplies" && <p>The rescue supplies crossed with you.</p>}
      </div>
    </section>
  );
}

/** Normalizes the participant action callback at the SDK's rendering boundary. */
function renderHotel(session: Snapshot | RegistrySession) {
  return "act" in session
    ? <GameView snapshot={session} />
    : <>
        <GameView snapshot={{ ...session, act: session.sendAction }} />
        <RegistryConversation session={session} />
      </>;
}

/** Keeps voice and text conversation available throughout play on the published SDK. */
function RegistryConversation({ session }: { session: RegistrySession }) {
  const [draft, setDraft] = useState("");
  const messages = session.conversation.filter((message) => message.input === "text").slice(-6);

  /** Sends one nonempty text message while the SDK can interact with the session. */
  function send(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const text = draft.trim();
    if (!session.interactionEnabled || !text) return;
    session.sendMessage(text);
    setDraft("");
  }

  return (
    <section className="parlando-session-tools registry-conversation" aria-label="Guest conversation">
      <div className="parlando-session-tools-header">
        <span>{session.connected ? "Guest conversation" : "Reconnecting…"}</span>
        <div className="parlando-session-tools-actions">
          {session.voiceEnabled && (
            <MicrophoneMuteButton enabled={session.connected} status={session.voiceStatus}
              onMutedChange={(muted) => { void session.setMicrophoneMuted(muted); }} />
          )}
          <button className="parlando-leave-session" type="button" onClick={session.leave}>Leave game</button>
        </div>
      </div>
      {messages.length > 0 && <ol className="parlando-session-tools-conversation" aria-label="Text conversation">
        {messages.map(({ id, sender, text }) => (
          <li className="parlando-session-tools-message" key={id}><strong>{sender}</strong><p>{text}</p></li>
        ))}
      </ol>}
      <form className="parlando-session-tools-form" onSubmit={send}>
        <input aria-label="Message" placeholder="Talk to the other guest" value={draft} disabled={!session.interactionEnabled} onChange={(event) => setDraft(event.target.value)} />
        <button type="submit" disabled={!session.interactionEnabled || !draft.trim()}>Send message</button>
      </form>
    </section>
  );
}
