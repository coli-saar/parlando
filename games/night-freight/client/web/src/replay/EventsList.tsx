/**
 * Events list: the whole run in plain language, hour by hour.
 *
 * Owns only presentation of `eventRows`: a heading per game hour, one line per
 * event, failures (abandoned steps, missed orders, holds, disruptions) in
 * warning colours and met orders in success colours. It is shown after the film
 * beside the "Back to planning" button and, as a "Last run" disclosure, in the
 * briefing panel during planning. The rows come from the actual trace, so both
 * roles read the same story.
 */
import type { GameMap, LastRun } from "../game";
import { clock12, stationName } from "../game";
import { eventRows } from "./narrate";

/** The events of a run, grouped by hour. */
export function EventsList({ run, map }: { run: LastRun; map: GameMap }) {
  const rows = eventRows(run, (id) => stationName(map, id));
  const hours = [...new Set(rows.map((r) => r.hour))];
  return (
    <section className="nf-events" aria-label="Events">
      <h3>Events</h3>
      <div className="nf-events-scroll">
        {hours.map((h) => (
          <div key={h} className="nf-events-hour">
            <div className="nf-events-clock">{clock12(h)}</div>
            <ul>
              {rows.filter((r) => r.hour === h).map((r, i) => (
                <li key={i} className={r.failure ? "fail" : r.good ? "good" : ""}>{r.failure ? "✗ " : r.good ? "✓ " : ""}{r.text}</li>
              ))}
            </ul>
          </div>
        ))}
      </div>
    </section>
  );
}
