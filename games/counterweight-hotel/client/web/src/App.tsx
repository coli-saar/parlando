import { useEffect } from "react";
import { ParticipantApp } from "@coli-saar/parlando-client/react";
import { GameView } from "./GameView";
import type { Action, Completion, Observation } from "./game";

import "./ending.css";

/** Provides the game surface and ending inside Parlando's participant lifecycle. */
export function App() {
  return (
    <ParticipantApp<Observation, Action, Completion>
      renderGame={(snapshot) => <GameView snapshot={snapshot} />}
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
