import { useState } from "react";
import { createRoot } from "react-dom/client";
import {
  ParticipantApp,
  type GameSession
} from "@coli-saar/parlando-client/react";
import "./style.css";

interface ContractObservation {
  role: "A" | "B";
  actions: number;
  done: boolean;
}

type ContractAction =
  | { type: "mark"; finish: boolean }
  | { type: "reject" };

interface ContractCompletion {
  done: boolean;
  actions: number;
}

/** Renders every reusable-client operation needed by the browser acceptance scenarios. */
function ContractSession({ session }: {
  session: GameSession<ContractObservation, ContractAction, ContractCompletion>;
}) {
  const [message, setMessage] = useState("");
  return (
    <main>
      <h1>Contract game</h1>
      <dl>
        <dt>Role</dt><dd data-testid="role">{session.role}</dd>
        <dt>Actions</dt><dd data-testid="actions">{session.observation.actions}</dd>
        <dt>Interaction</dt><dd data-testid="interaction">{session.interactionEnabled ? "enabled" : "paused"}</dd>
        <dt>Messages</dt><dd data-testid="messages">{session.conversation.map((item) => item.text).join(" | ") || "none"}</dd>
        <dt>Voice</dt><dd data-testid="voice">{session.voiceStatus.message}</dd>
        <dt>Remote audio</dt><dd data-testid="remote-audio">{session.voiceStatus.remoteAudio ? "heard" : "silent"}</dd>
      </dl>
      <div className="actions">
        <button onClick={() => session.sendAction({ type: "mark", finish: false })}>Advance</button>
        <button onClick={() => session.sendAction({ type: "reject" })}>Reject action</button>
        <button onClick={() => session.sendAction({ type: "mark", finish: true })}>Finish game</button>
        <button onClick={session.leave}>Leave game</button>
      </div>
      <label>
        Message
        <input aria-label="Message" onChange={(event) => setMessage(event.target.value)} value={message} />
      </label>
      <button onClick={() => {
        session.sendMessage(message);
        setMessage("");
      }}>Send message</button>
    </main>
  );
}

/** Renders the fixture through the public package surface used by real games. */
function App() {
  return (
    <ParticipantApp<ContractObservation, ContractAction, ContractCompletion>
      renderGame={(session) => <ContractSession session={session} />}
      renderCompletion={(completion) => (
        <p data-testid="completion">Completed after {completion.actions} actions</p>
      )}
    />
  );
}

const root = document.getElementById("root");
if (!root) throw new Error("fixture root is missing");
createRoot(root).render(<App />);
