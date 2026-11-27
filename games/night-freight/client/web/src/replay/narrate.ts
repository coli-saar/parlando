/**
 * Narration: plain-language sentences for the events of a run.
 *
 * Owns every sentence the film captions and the "Events" list show: what an
 * engine does ("E1 couples 1 empty boxcar at Dansville"), why a step never ran
 * (built from the wire `FailReason`), and why an order was missed (built from
 * `MissReason`). Pure functions of an event and the shared map names; no state.
 * Callers rely on `narrate` returning a sentence without a clock prefix and on
 * `isFailure` marking exactly the events that should be highlighted as failures.
 */
import type { CarCount, CarKind, FailReason, GameEvent, LastRun, LoadFilter, MissReason, Op } from "../game";
import { clock12, describeCars, plural } from "../game";

/** Station display name lookup. */
export type Names = (id: string) => string;

/** English list of car groups: "1 empty boxcar and 2 boxcars of oranges", or "no boxcars". */
function carCounts(kind: CarKind | null, list: CarCount[]): string {
  if (!list.length) return kind ? `no ${kind}s` : "no cars";
  const parts = list.map((c) => {
    const noun = plural(c.count, kind ?? "car");
    return c.load === null ? noun.replace(" ", " empty ") : `${noun} of ${c.load}`;
  });
  return parts.length === 1 ? parts[0] : `${parts.slice(0, -1).join(", ")} and ${parts[parts.length - 1]}`;
}

/** The car group of a couple or uncouple step as the sentence needs it ("1 boxcar of oranges"). */
function carsOf(op: Extract<Op, { type: "couple" | "uncouple" }>): string { return describeCars(op.kind, op.load, op.count); }

/** Verb phrase of a step in the base form, for "couldn't …" and "starts …": "couple 1 boxcar". */
function verbPhrase(op: Op): string {
  switch (op.type) {
    case "couple": return `couple ${carsOf(op)}`;
    case "uncouple": return `uncouple ${carsOf(op)}`;
    case "load": return `load ${plural(op.count, "car")} with ${op.commodity}`;
    case "process": return `process ${plural(op.count, "car")} at the factory`;
    case "deliver": return `deliver ${op.order}`;
  }
}

/** Third-person present tense of a step: "couples 1 boxcar", "loads 1 car with oranges". */
function thirdPerson(op: Op): string {
  const v = verbPhrase(op);
  return v.replace(/^(\w+)/, (w) => (w === "process" ? "processes" : `${w}s`));
}

/** The reason a step never ran, as the part of the sentence after the colon. */
function reasonText(r: FailReason, names: Names, station: string): string {
  const here = names(station);
  switch (r.type) {
    case "no_matching_cars_in_yard": return `the yard has ${carCounts(r.kind, r.yard)}.`;
    case "over_capacity": return `it already pulls ${plural(r.carried, "car")} and can pull at most ${r.capacity}.`;
    case "no_matching_cars_on_engine": return `it carries ${carCounts(r.kind, r.coupled)}.`;
    case "no_stock_here": return `${here} has no ${r.commodity} in stock.`;
    case "no_empty_cars_on_engine": return `it pulls ${plural(r.found, `empty ${r.kind}`)}, but needs ${r.wanted}.`;
    case "no_factory_here": return `there is no factory at ${here}.`;
    case "no_input_cars": return `it pulls ${plural(r.found, "car")} of ${r.load ?? "the factory's input"}, but needs ${r.wanted}.`;
    case "wrong_destination": return `${r.order} goes to ${names(r.to)}.`;
    case "no_order_loads_on_engine": return `no car of ${r.commodity} is on the engine; it carries ${carCounts(null, r.coupled)}.`;
    case "order_filled": return `${r.order} is already complete.`;
    case "unknown_order": return `there is no order ${r.order}.`;
  }
}

/** Sentence for an abandoned step: "E1 couldn't couple 1 boxcar of oranges at Corning: the yard has 1 empty boxcar." */
export function describeAbandon(k: Extract<GameEvent["kind"], { type: "op_abandoned" }>, names: Names): string {
  return `${k.engine} couldn't ${verbPhrase(k.op)} at ${names(k.station)}: ${reasonText(k.reason, names, k.station)}`;
}

/** Sentence for a missed order: "O1 missed: no car of oranges reached Dansville by 5:00 am." */
export function describeMiss(order: string, why: MissReason, deadline: number | null, names: Names): string {
  const to = names(why.to);
  const dl = deadline === null ? "the deadline" : `the ${clock12(deadline)} deadline`;
  switch (why.type) {
    case "no_car_reached": return `${order} missed: no car of ${why.commodity} reached ${to}${deadline === null ? "" : ` by ${clock12(deadline)}`}.`;
    case "reached_after_deadline": return `${order} missed: a car of ${why.commodity} reached ${to} at ${clock12(why.reached_at)}, after ${dl}.`;
    case "reached_not_delivered": return `${order} missed: a car of ${why.commodity} reached ${to} at ${clock12(why.reached_at)} but was never delivered (${why.delivered} of ${why.count} delivered).`;
    case "delivered_after_deadline": return `${order} missed: it was delivered at ${clock12(why.delivered_at)}, after ${dl}.`;
  }
}

/** Plain sentence for one trace event (no clock prefix); `deadlines` maps order ids to deadline hours. */
export function narrate(ev: GameEvent, names: Names, deadlines: Map<string, number> = new Map()): string {
  const k = ev.kind;
  switch (k.type) {
    case "arrive": return `${k.engine} arrives at ${names(k.station)}`;
    case "depart": return `${k.engine} leaves ${names(k.from)} for ${names(k.to)}, arriving at ${clock12(k.arrive_hour)}`;
    case "hold": return k.reason === "closed" ? `${k.engine} is held at ${names(k.station)}: the line ahead is closed` : `${k.engine} waits at ${names(k.station)} for a clear line`;
    case "op_started": {
      const what = k.op.type === "load" ? `loading ${k.op.commodity}` : k.op.type === "process" ? "processing at the factory" : k.op.type === "deliver" ? `delivering ${k.op.order}` : verbPhrase(k.op);
      return `${k.engine} starts ${what} at ${names(k.station)}, done at ${clock12(k.finish_hour)}`;
    }
    case "op_done": return `${k.engine} ${thirdPerson(k.op)} at ${names(k.station)}`;
    case "op_abandoned": return describeAbandon(k, names);
    case "op_unreached": return `${k.engine} never gets to "${verbPhrase(k.op)}" at ${names(k.station)}: the route does not visit it that many times`;
    case "order_met": return `${k.order} is met`;
    case "order_missed": return describeMiss(k.order, k.why, deadlines.get(k.order) ?? null, names);
    case "disrupted":
      return k.what.type === "engine_failure" ? `${k.what.engine} has failed at ${names(k.what.station)}`
        : k.what.type === "track_closure" ? `The line ${names(k.what.a)} to ${names(k.what.b)} is closed`
          : "A telegram arrives at the depot: the orders change";
  }
}

/** True for events the Events list highlights as failures. */
export function isFailure(ev: GameEvent): boolean {
  return ev.kind.type === "op_abandoned" || ev.kind.type === "order_missed" || ev.kind.type === "op_unreached" || ev.kind.type === "hold" || ev.kind.type === "disrupted";
}

/** One row of the Events list. */
export interface EventRow { hour: number; text: string; failure: boolean; good: boolean }

/** Rows of the whole run, chronological, built from the actual trace. */
export function eventRows(run: LastRun, names: Names): EventRow[] {
  const deadlines = new Map(run.actual.initial.orders.map((o) => [o.id, o.deadline]));
  return run.actual.events.map((ev) => ({
    hour: ev.hour, text: narrate(ev, names, deadlines), failure: isFailure(ev), good: ev.kind.type === "order_met"
  }));
}

/** English for a load filter ("any", "empty", "loaded with oranges"), used by the picker summary. */
export function describeFilter(f: LoadFilter): string {
  return f.type === "any" ? "any" : f.type === "empty" ? "empty" : `loaded with ${f.commodity}`;
}

/** Sentence for the end of a timed op: "E1 finished loading oranges at Corning". */
export function finishedText(engine: string, op: Op, station: string, names: Names): string {
  const what = op.type === "load" ? `loading ${op.commodity}` : op.type === "process" ? "processing" : op.type === "deliver" ? `delivering ${op.order}` : verbPhrase(op);
  return `${engine} finished ${what} at ${names(station)}`;
}

/** Label of a running timed op for the progress card: "Loading oranges · 1 h". */
export function workLabel(op: Op, hours: number): string {
  const what = op.type === "load" ? `Loading ${op.commodity}` : op.type === "process" ? "Processing at the factory" : op.type === "deliver" ? `Delivering ${op.order}` : "Shunting";
  return hours > 0 ? `${what} · ${hours} h` : what;
}
