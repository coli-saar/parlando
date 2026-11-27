/**
 * Briefing panel: the viewer's private facts, per role.
 *
 * Freight sees orders, the cars standing in each yard, stock, factories and the
 * handling durations. Routing sees the travel time of every edge against the
 * clock, track restrictions and engine capacities. Either role sees the
 * announcement of a disruption addressed to them (and only that role does; the
 * partner must be told). The panel renders straight from `observation.briefing`.
 */
import type { Car, GameMap, Observation } from "../game";
import { clock, stationName } from "../game";
import { EventsList } from "../replay/EventsList";

/** Groups yard cars by station for the yard list. */
function yards(cars: Car[], map: GameMap): { station: string; text: string }[] {
  const byStation = new Map<string, Car[]>();
  for (const c of cars) if (c.at.type === "yard") byStation.set(c.at.station, [...(byStation.get(c.at.station) ?? []), c]);
  return map.stations.filter((s) => byStation.has(s.id)).map((s) => {
    const list = byStation.get(s.id)!;
    const count = (k: string) => list.filter((c) => c.kind === k).length;
    const parts = [count("boxcar") && `${count("boxcar")} boxcar${count("boxcar") > 1 ? "s" : ""}`, count("tanker") && `${count("tanker")} tanker${count("tanker") > 1 ? "s" : ""}`].filter(Boolean);
    const loaded = list.filter((c) => c.load).map((c) => c.load).join(", ");
    return { station: s.id, text: `${parts.join(" and ")}${loaded ? ` (loaded: ${loaded})` : ""}` };
  });
}

/** Renders the role's briefing. */
export function Briefing({ observation: o }: { observation: Observation }) {
  const b = o.briefing;
  const map = o.map;
  return (
    <section className="nf-briefing" aria-label="Your briefing">
      <h2>{o.scenario_title}</h2>
      <p className="nf-muted">{o.scenario_summary}</p>
      {o.last_run && (
        <details className="nf-lastrun">
          <summary>Last run (run {o.last_run.number}{o.last_run.success ? ", all orders met" : ""})</summary>
          <EventsList run={o.last_run} map={map} />
        </details>
      )}
      {b.announcement && <div className="nf-announce" role="alert"><strong>News from the line.</strong> {b.announcement.message}</div>}
      {b.type === "freight" ? (
        <>
          <h3>Orders</h3>
          <ul>
            {b.orders.map((ord) => (
              <li key={ord.id} className={ord.delivered >= ord.count ? "done" : ""}>
                <strong>{ord.id}</strong> {ord.count} car{ord.count > 1 ? "s" : ""} of {ord.commodity} to {stationName(map, ord.to)} by {clock(ord.deadline)}
                {ord.delivered >= ord.count && ` (delivered ${ord.delivered_at !== null ? clock(ord.delivered_at) : ""})`}
              </li>
            ))}
          </ul>
          <h3>Cars in the yards</h3>
          <ul>{yards(b.cars, map).map((y) => <li key={y.station}><strong>{stationName(map, y.station)}</strong>: {y.text}</li>)}</ul>
          <h3>Stock and factories</h3>
          <ul>
            {b.stock.map((s) => <li key={s.station}>{stationName(map, s.station)} stocks {s.commodities.join(", ")}</li>)}
            {b.factories.map((f) => <li key={f.station}>{stationName(map, f.station)} turns {f.input} into {f.output} in {f.hours} h</li>)}
          </ul>
          <h3>Handling times</h3>
          <p>Loading takes {b.load_hours} h per batch; delivering takes {b.deliver_hours} h. Coupling and uncoupling are instant. A train leaves a stop only after its cargo steps there, in whatever order works.</p>
        </>
      ) : (
        <>
          <h3>Travel times</h3>
          <ul>{b.travel_times.map((t) => <li key={`${t.a}${t.b}`}>{stationName(map, t.a)} – {stationName(map, t.b)}: <strong>{t.hours} h</strong></li>)}</ul>
          <h3>Track restrictions</h3>
          {b.restrictions.length ? (
            <ul>{b.restrictions.map((r, i) => (
              <li key={i}>{stationName(map, r.a)} – {stationName(map, r.b)} closed to entering {clock(r.from_hour)} until {r.until_hour > 24 ? "the end of the day" : clock(r.until_hour)}</li>
            ))}</ul>
          ) : <p className="nf-muted">None.</p>}
          <h3>Engines</h3>
          <ul>
            {b.capacities.map((c) => {
              const e = map.engines.find((x) => x.id === c.engine);
              return <li key={c.engine}><strong>{c.engine}</strong> pulls up to {c.capacity} cars{e?.failed ? ", failed" : e?.available_from ? `, available from ${clock(e.available_from)}` : ""}</li>;
            })}
          </ul>
          <p className="nf-muted">One train at a time on each single-track edge.</p>
        </>
      )}
    </section>
  );
}
