import { createRoot } from "react-dom/client";
import {
  CommunicationWidget,
  ParticipantApp,
  type GameSnapshot
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

/** Renders the game-owned snapshot and actions used by the browser acceptance scenarios. */
function ContractGame({ snapshot, placeCommunication }: {
  snapshot: GameSnapshot<ContractObservation, ContractAction>;
  placeCommunication: boolean;
}) {
  return (
    <main>
      <h1>Contract game</h1>
      <dl>
        <dt>Role</dt><dd data-testid="role">{snapshot.role}</dd>
        <dt>Actions</dt><dd data-testid="actions">{snapshot.observation.actions}</dd>
        <dt>Interaction</dt><dd data-testid="interaction">{snapshot.interactionEnabled ? "enabled" : "paused"}</dd>
      </dl>
      <div className="actions">
        <button onClick={() => snapshot.act({ type: "mark", finish: false })}>Advance</button>
        <button onClick={() => snapshot.act({ type: "reject" })}>Reject action</button>
        <button onClick={() => snapshot.act({ type: "mark", finish: true })}>Finish game</button>
      </div>
      {placeCommunication && <aside data-testid="game-communication"><CommunicationWidget /></aside>}
    </main>
  );
}

/** Renders the fixture through the public package surface used by real games. */
function App() {
  const placeCommunication = new URLSearchParams(window.location.search).has("gameCommunication");
  return (
    <ParticipantApp<ContractObservation, ContractAction, ContractCompletion>
      communicationPlacement={placeCommunication ? "game" : undefined}
      renderGame={(snapshot) => <ContractGame placeCommunication={placeCommunication} snapshot={snapshot} />}
      renderCompletion={(completion) => (
        <p data-testid="completion">Completed after {completion.actions} actions</p>
      )}
    />
  );
}

const root = document.getElementById("root");
if (!root) throw new Error("fixture root is missing");
createRoot(root).render(<App />);
