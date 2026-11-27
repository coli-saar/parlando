/**
 * Notes editor: one strip per engine, stops left to right, as in the approved
 * notes sketch.
 *
 * Owns the presentation of the viewer's own `Plan` and the translation of each
 * gesture into one backend `edit` action: "+" appends an adjacent station, a stop
 * menu removes it and everything after it, sets the departure rule or the heard
 * arrival time, and a step "+" opens the picker for the five cargo op kinds.
 * Each stop card lists its cargo steps first and the departure last, worded as
 * happening after the steps (the engine leaves only once no step can start).
 * Fields the viewer's role owns are solid (Freight: cargo steps; Routing: route
 * and departure), partner-owned fields are dashed, and empty partner-owned
 * fields read "?". The editor keeps no copy of the notes: every render comes from
 * the view, so it cannot diverge from the server. Partner `edit` payloads never
 * reach this component (spec §15.1: they are ignored by the app).
 *
 * Freight does not see this strip. Routing alone decides where and when the
 * trains go, so Freight's notes are a wrapping row of separate place cards (an
 * itinerary of where cargo work happens) with no arrows, departures or route
 * fields; see `FreightRow`.
 */
import type { Action, Depart, EngineId, GameMap, Observation, Op, Stop } from "../game";
import { clock, describeDepart, describeOp, edit, stationName } from "../game";
import { OpPicker, stopFacts } from "./OpPicker";
import { Popover } from "./Popover";

/** Props of the editor. */
export interface NotesProps {
  observation: Observation;
  enabled: boolean;
  act(action: Action): void;
}

/** Whether a role owns the route (Routing) or the cargo steps (Freight). */
const ownsRoute = (role: Observation["role"]) => role === "routing";

/** Menu body that sets the departure rule of a stop. */
function DepartMenu({ engine, stop, depart, engines, close, act }: {
  engine: EngineId; stop: number; depart: Depart; engines: EngineId[]; close(): void; act(a: Action): void;
}) {
  const send = (d: Depart) => { act(edit({ type: "set_depart", engine, stop, depart: d })); close(); };
  const others = engines.filter((e) => e !== engine);
  const after = depart.type === "after" ? depart : { type: "after" as const, engine: others[0] ?? engine, plus_hours: 1 };
  return (
    <div className="nf-menu">
      <button type="button" onClick={() => send({ type: "immediately" })}>Leave as soon as its steps are done</button>
      <label>Earliest departure (after its steps)
        <select value={depart.type === "at" ? depart.hour : ""} onChange={(e) => e.target.value !== "" && send({ type: "at", hour: Number(e.target.value) })}>
          <option value="">choose…</option>
          {Array.from({ length: 25 }, (_, h) => <option key={h} value={h}>{clock(h)}</option>)}
        </select>
      </label>
      {others.length > 0 && (
        <label>Earliest departure: some hours after another engine arrives
          <span className="nf-inline">
            <select value={after.plus_hours} onChange={(e) => send({ ...after, plus_hours: Number(e.target.value) })} aria-label="Hours after">
              {Array.from({ length: 13 }, (_, h) => <option key={h} value={h}>{h} h after</option>)}
            </select>
            <select value={after.engine} onChange={(e) => send({ ...after, engine: e.target.value })} aria-label="Engine">
              {others.map((e) => <option key={e} value={e}>{e}</option>)}
            </select>
          </span>
        </label>
      )}
    </div>
  );
}

/** One stop column: header chip, departure chip, cargo steps and the step "+". */
function StopColumn({ p, engine, index, stop, last, engines, map }: {
  p: NotesProps; engine: EngineId; index: number; stop: Stop; last: boolean; engines: EngineId[]; map: GameMap;
}) {
  const routeMine = ownsRoute(p.observation.role);
  const facts = stopFacts(p.observation, stop.station, map);
  const act = p.act;
  const heard = stop.heard_arrival;
  return (
    <div className="nf-stop">
      <div className={`nf-chip nf-head ${routeMine ? "mine" : "theirs"}`}>
        <span className="nf-station">{stationName(map, stop.station)}</span>
        <Popover className="nf-heard" title="Arrival time you heard" disabled={!p.enabled}
          label={heard === null ? "?" : clock(heard)}>
          {(close) => (
            <div className="nf-menu">
              <label>Heard arrival
                <select value={heard ?? ""} onChange={(e) => { act(edit({ type: "set_heard_arrival", engine, stop: index, hour: e.target.value === "" ? null : Number(e.target.value) })); close(); }}>
                  <option value="">not known (?)</option>
                  {Array.from({ length: 25 }, (_, h) => <option key={h} value={h}>{clock(h)}</option>)}
                </select>
              </label>
            </div>
          )}
        </Popover>
        {index > 0 && (
          <Popover className="nf-more" title="Stop menu" disabled={!p.enabled} label="⋯">
            {(close) => (
              <div className="nf-menu">
                <button type="button" onClick={() => { act(edit({ type: "truncate_stops", engine, from: index })); close(); }}>Remove from here on</button>
              </div>
            )}
          </Popover>
        )}
      </div>
      {stop.ops.map((op: Op, i) => (
        <div key={i} className={`nf-chip nf-op ${routeMine ? "theirs" : "mine"}`}>
          <span>{describeOp(op)}</span>
          <button type="button" className="nf-x" aria-label={`Remove step ${describeOp(op)}`} disabled={!p.enabled}
            onClick={() => act(edit({ type: "remove_op", engine, stop: index, index: i }))}>×</button>
        </div>
      ))}
      {stop.ops.length === 0 && routeMine && <div className="nf-chip nf-op theirs nf-unknown" title="Cargo steps are Freight's part, unknown to you">?</div>}
      <Popover className="nf-add" title="Add cargo step" disabled={!p.enabled} label="+ step">
        {(close) => <OpPicker facts={facts} onAdd={(op) => { act(edit({ type: "insert_op", engine, stop: index, index: stop.ops.length, op })); close(); }} />}
      </Popover>
      <Popover className={`nf-chip nf-depart ${routeMine ? "mine" : "theirs"}`} disabled={!p.enabled}
        title="Departure" label={last && index > 0 ? "Final stop · stays here" : describeDepart(stop.depart)}>
        {(close) => <DepartMenu engine={engine} stop={index} depart={stop.depart} engines={engines} close={close} act={act} />}
      </Popover>
    </div>
  );
}

/** "1st", "2nd", "3rd", "4th", ... */
function ordinal(n: number): string {
  const tail = n % 100 >= 11 && n % 100 <= 13 ? "th" : ({ 1: "st", 2: "nd", 3: "rd" } as Record<number, string>)[n % 10] ?? "th";
  return `${n}${tail}`;
}

/**
 * Removes one place (stop `index` > 0) with the existing edits: truncate from
 * that stop, then re-append the later stops and re-insert their cargo steps.
 * Several edits are sent in order; the wire format has no single-stop removal.
 */
function removePlace(act: (a: Action) => void, engine: EngineId, stops: Stop[], index: number) {
  act(edit({ type: "truncate_stops", engine, from: index }));
  stops.slice(index + 1).forEach((stop, k) => {
    const at = index + k;
    act(edit({ type: "append_stop", engine, station: stop.station }));
    stop.ops.forEach((op, i) => act(edit({ type: "insert_op", engine, stop: at, index: i, op })));
  });
}

/** One Freight place card: "At <station>" with its cargo steps, "+ step" and "remove place". */
function PlaceCard({ p, engine, stops, index }: { p: NotesProps; engine: EngineId; stops: Stop[]; index: number }) {
  const map = p.observation.map;
  const stop = stops[index];
  const facts = stopFacts(p.observation, stop.station, map);
  const total = stops.filter((s) => s.station === stop.station).length;
  const nth = stops.slice(0, index + 1).filter((s) => s.station === stop.station).length;
  const tag = index === 0 ? " (start)" : total > 1 ? ` (${ordinal(nth)} visit)` : "";
  return (
    <div className="nf-stop nf-place">
      <div className="nf-chip nf-head mine">
        <span className="nf-station">At {stationName(map, stop.station)}{tag}</span>
      </div>
      {stop.ops.map((op: Op, i) => (
        <div key={i} className="nf-chip nf-op mine">
          <span>{describeOp(op)}</span>
          <button type="button" className="nf-x" aria-label={`Remove step ${describeOp(op)}`} disabled={!p.enabled}
            onClick={() => p.act(edit({ type: "remove_op", engine, stop: index, index: i }))}>×</button>
        </div>
      ))}
      <Popover className="nf-add" title="Add cargo step" disabled={!p.enabled} label="+ step">
        {(close) => <OpPicker facts={facts} onAdd={(op) => { p.act(edit({ type: "insert_op", engine, stop: index, index: stop.ops.length, op })); close(); }} />}
      </Popover>
      {index > 0 && (
        <button type="button" className="nf-remove-place" disabled={!p.enabled} onClick={() => removePlace(p.act, engine, stops, index)}>remove place</button>
      )}
    </div>
  );
}

/** Freight's notes for one engine: separate place cards in a wrapping row, plus "+ place" for any station. */
function FreightRow({ p, id, stops }: { p: NotesProps; id: EngineId; stops: Stop[] }) {
  const o = p.observation;
  return (
    <div className="nf-row">
      <div className="nf-eng">{id}</div>
      <div className="nf-places">
        {stops.map((_, i) => <PlaceCard key={i} p={p} engine={id} stops={stops} index={i} />)}
        <Popover className="nf-plus" title="Add a place" disabled={!p.enabled || stops.length >= 12} label="+ place">
          {(close) => (
            <div className="nf-menu">
              <div className="nf-menu-title">Where does something happen?</div>
              {o.map.stations.map((s) => (
                <button key={s.id} type="button" onClick={() => { p.act(edit({ type: "append_stop", engine: id, station: s.id })); close(); }}>{s.name}</button>
              ))}
            </div>
          )}
        </Popover>
      </div>
    </div>
  );
}

/** The editor for all engine strips plus the legend and Ready button. */
export function NotesEditor(p: NotesProps) {
  const { observation: o, enabled, act } = p;
  const engines = Object.keys(o.notes.engines).sort();
  const adjacent = (station: string) => o.map.edges.flatMap((e) => (e.a === station ? [e.b] : e.b === station ? [e.a] : []));
  const mineSolid = ownsRoute(o.role) ? "your route and departures" : "what happens at each place";
  const partnerName = o.role === "freight" ? "Routing" : "Freight";
  const ready = o.ready;
  return (
    <section className="nf-notes" aria-label="Your notes" data-tutorial="notes">
      <div className="nf-strips">
        {engines.map((id) => {
          const stops = o.notes.engines[id];
          if (o.role === "freight") return <FreightRow key={id} p={p} id={id} stops={stops} />;
          return (
            <div className="nf-row" key={id}>
              <div className="nf-eng">{id}</div>
              <div className="nf-strip">
                {stops.map((stop, i) => (
                  <div className="nf-cell" key={i}>
                    {i > 0 && <span className="nf-arrow" aria-hidden="true">→</span>}
                    <StopColumn p={p} engine={id} index={i} stop={stop} last={i === stops.length - 1} engines={engines} map={o.map} />
                  </div>
                ))}
                <div className="nf-cell">
                  <span className="nf-arrow" aria-hidden="true">→</span>
                  <Popover className="nf-plus" title="Append a station" disabled={!enabled || stops.length >= 12} label="+">
                    {(close) => (
                      <div className="nf-menu">
                        <div className="nf-menu-title">Next stop from {stationName(o.map, stops[stops.length - 1].station)}</div>
                        {adjacent(stops[stops.length - 1].station).map((s) => (
                          <button key={s} type="button" onClick={() => { act(edit({ type: "append_stop", engine: id, station: s })); close(); }}>{stationName(o.map, s)}</button>
                        ))}
                      </div>
                    )}
                  </Popover>
                </div>
              </div>
            </div>
          );
        })}
      </div>
      <p className="nf-order-note">{o.role === "freight"
        ? "Routing decides where and when the trains go. You say what happens at each place; steps at a place happen in whatever order works."
        : "Cargo steps at a stop happen in whatever order works; the train leaves only after them."}</p>
      <div className="nf-legend">
        <span><span className="nf-chip mine nf-key">Solid</span> {mineSolid}, used in the run</span>
        {o.role === "routing" && <span><span className="nf-chip theirs nf-key">Dashed</span> what you think {partnerName} will do</span>}
        {o.role === "freight" && <span>{partnerName} decides where and when the trains go</span>}
        <span className="nf-spacer" />
        <span className={`nf-partner ${ready.partner ? "on" : ""}`} role="status">{partnerName} is {ready.partner ? "ready" : "not ready"}</span>
        <button type="button" className={`nf-primary nf-ready${ready.me ? " on" : ""}`} data-tutorial="ready" disabled={!enabled}
          aria-pressed={ready.me} onClick={() => act({ type: "set_ready", ready: !ready.me })}>
          {ready.me ? (ready.partner ? "Starting…" : "Ready · waiting (click to withdraw)") : "▶ Ready to run"}
        </button>
      </div>
    </section>
  );
}
