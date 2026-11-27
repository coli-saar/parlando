/**
 * Scenario-done screen: shows the finished scenario's score and the earlier
 * ones, then waits for both players to press Continue.
 *
 * Owns presentation only. The last score is the one the server appended for
 * this scenario; orders met and runs are shown side by side and never combined.
 */
import type { Action, Observation } from "../game";

/** The result screen between scenarios. */
export function ScenarioDone({ observation: o, enabled, act }: { observation: Observation; enabled: boolean; act(a: Action): void }) {
  const last = o.scores[o.scores.length - 1];
  const failed = !!last && last.orders_met < last.orders_total;
  return (
    <section className="nf-done" aria-labelledby="nf-done-title">
      <p className="nf-eyebrow">Scenario {o.scenario_index + 1} of {o.scenario_count}</p>
      <h2 id="nf-done-title">{failed ? "The day is over" : "All orders delivered"}</h2>
      <p className="nf-muted">{o.scenario_title}</p>
      {last && (
        <div className="nf-score-big">
          <div><span className="nf-num">{last.orders_met}/{last.orders_total}</span><span>orders met</span></div>
          <div><span className="nf-num">{last.runs_used}</span><span>run{last.runs_used === 1 ? "" : "s"} used</span></div>
          <div><span className="nf-num">{last.replanned ? "yes" : "no"}</span><span>replanned after the disruption</span></div>
        </div>
      )}
      {o.scores.length > 1 && (
        <table className="nf-scores"><thead><tr><th>Scenario</th><th>Orders met</th><th>Runs</th></tr></thead>
          <tbody>{o.scores.map((s) => <tr key={s.scenario}><td>{s.scenario}</td><td>{s.orders_met}/{s.orders_total}</td><td>{s.runs_used}</td></tr>)}</tbody></table>
      )}
      <button type="button" className="nf-primary" disabled={!enabled || o.continue.me} onClick={() => act({ type: "continue" })}>
        {o.continue.me ? "Waiting for your partner…" : "Continue"}
      </button>
    </section>
  );
}
