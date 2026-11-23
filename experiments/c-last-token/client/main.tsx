import { ParticipantApp, type GameSnapshot } from "@coli-saar/parlando-client/react";
import { createRoot } from "react-dom/client";
import "./style.css";

type View = { remaining: number; your_turn: boolean };
type Action = { type: "take_one" | "take_two" | "take_three" };
type Completion = { winner: "A" | "B" };
const moves = ["take_one", "take_two", "take_three"] as const;

/** Render the role's view and send selected moves through the participant SDK. */
function Pile({ observation, interactionEnabled, act }: GameSnapshot<View, Action>) {
  const { remaining, your_turn: yourTurn } = observation;

  return (
    <section className="pile">
      <h1>Last Token</h1>
      <p>Take up to three tokens. Whoever takes the last token wins.</p>
      <p aria-live="polite">
        <strong>{remaining} {remaining === 1 ? "token" : "tokens"} left</strong>
        {yourTurn ? " · Your turn" : " · Partner’s turn"}
      </p>
      <div className="moves">
        {moves.map((type, index) => (
          <button
            key={type}
            disabled={!interactionEnabled || !yourTurn || remaining <= index}
            onClick={() => act({ type })}
          >
            Take {index + 1}
          </button>
        ))}
      </div>
    </section>
  );
}

createRoot(document.getElementById("root")!).render(
  <ParticipantApp<View, Action, Completion>
    renderGame={(snapshot) => <Pile {...snapshot} />}
    renderCompletion={({ winner }) => <p>Player {winner} took the last token and wins.</p>}
  />,
);
