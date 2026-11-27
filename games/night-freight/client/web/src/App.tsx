/**
 * App: provides the Night Freight game surface and ending inside Parlando's
 * participant lifecycle.
 *
 * The communication widget is placed inside the game layout
 * (`communicationPlacement="game"`), so the game view renders it itself.
 */
import { ParticipantApp } from "@coli-saar/parlando-client/react";
import { GameView } from "./GameView";
import type { Action, Completion, Observation } from "./game";

/** Final screen shown when both players have continued past the last scenario. */
export function Ending({ completion }: { completion: Completion }) {
  return (
    <section className="nf-done nf-ending" aria-labelledby="nf-end-title">
      <p className="nf-eyebrow">Night Freight</p>
      <h2 id="nf-end-title">The night shift is over</h2>
      <table className="nf-scores">
        <thead><tr><th>Scenario</th><th>Orders met</th><th>Runs</th><th>Replanned</th></tr></thead>
        <tbody>
          {completion.scores.map((s) => (
            <tr key={s.scenario}><td>{s.scenario}</td><td>{s.orders_met}/{s.orders_total}</td><td>{s.runs_used}</td><td>{s.replanned ? "yes" : "no"}</td></tr>
          ))}
        </tbody>
      </table>
    </section>
  );
}

/** Root component of the browser client. */
export function App() {
  return (
    <ParticipantApp<Observation, Action, Completion>
      communicationPlacement="game"
      renderGame={(snapshot) => <GameView snapshot={snapshot as never} />}
      renderCompletion={(completion) => <Ending completion={completion} />}
    />
  );
}
