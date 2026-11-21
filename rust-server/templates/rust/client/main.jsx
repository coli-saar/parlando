import React from "react";
import { createRoot } from "react-dom/client";
import { ParticipantApp } from "@coli-saar/parlando-client/react";

/** Renders the server's current observation and submits typed pulses. */
function Beacon({ snapshot }) {
  const { observation, interactionEnabled, act } = snapshot;
  const { charge, target, your_turn: yourTurn } = observation;
  const enabled = interactionEnabled && yourTurn;
  return (
    <section aria-labelledby="beacon-name">
      <h1 id="beacon-name">Beacon Relay</h1>
      <p>Take turns charging the beacon. Reach the target exactly to activate it.</p>
      <label htmlFor="charge">Charge {charge} / {target}</label>{" "}
      <progress id="charge" max={target} value={charge} />
      <p role="status">{yourTurn ? "Your turn" : "Waiting for your partner"}</p>
      <button disabled={!enabled || charge + 1 > target} onClick={() => act({ ticks: 1 })}>
        Add one tick
      </button>{" "}
      <button disabled={!enabled || charge + 2 > target} onClick={() => act({ ticks: 2 })}>
        Add two ticks
      </button>
    </section>
  );
}

createRoot(document.getElementById("root")).render(
  <ParticipantApp
    renderGame={(snapshot) => <Beacon snapshot={snapshot} />}
    renderCompletion={(outcome) => <p>Beacon activated at {outcome.activated_at} ticks.</p>}
  />
);
