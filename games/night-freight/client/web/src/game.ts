/**
 * Wire types for Night Freight, mirroring `server/WIRE-FORMAT.md` exactly.
 *
 * Everything here describes JSON that the Rust component produces (views, traces)
 * or accepts (actions). Keys are snake_case, enums with data are internally tagged
 * with `type`, optional values are `null` and never omitted. The helpers at the
 * bottom construct actions and format a few shared values; they hold no state.
 */

/** The two roles; which Parlando seat plays which is server configuration. */
export type Role = "freight" | "routing";
export type EngineId = string;
export type StationId = string;
export type OrderId = string;
export type CarKind = "boxcar" | "tanker";
export type Commodity = "oranges" | "bananas" | "juice";

/** Departure rule of a stop (owned by Routing). */
export type Depart =
  | { type: "immediately" }
  | { type: "at"; hour: number }
  | { type: "after"; engine: EngineId; plus_hours: number };

/** Which cars a couple/uncouple step takes: any car, only empty cars, or only cars loaded with a commodity. */
export type LoadFilter = { type: "any" } | { type: "empty" } | { type: "loaded"; commodity: Commodity };

/**
 * A cargo step at a stop (owned by Freight). The engine starts, whenever it is
 * free, the first step whose precondition holds, so list order does not matter;
 * the engine leaves only after its steps.
 */
export type Op =
  | { type: "couple"; kind: CarKind; load: LoadFilter; count: number }
  | { type: "uncouple"; kind: CarKind; load: LoadFilter; count: number }
  | { type: "load"; commodity: Commodity; count: number }
  | { type: "process"; count: number }
  | { type: "deliver"; order: OrderId };

/** One station visit in an engine's strip. */
export interface Stop {
  station: StationId;
  depart: Depart;
  ops: Op[];
  heard_arrival: number | null;
}

/** The notes of one role: every engine always has a strip, `[0]` is fixed. */
export interface Plan { engines: Record<EngineId, Stop[]> }

/** Edits the server accepts on the actor's own notes. */
export type NoteEdit =
  | { type: "append_stop"; engine: EngineId; station: StationId }
  | { type: "truncate_stops"; engine: EngineId; from: number }
  | { type: "set_depart"; engine: EngineId; stop: number; depart: Depart }
  | { type: "insert_op"; engine: EngineId; stop: number; index: number; op: Op }
  | { type: "remove_op"; engine: EngineId; stop: number; index: number }
  | { type: "set_heard_arrival"; engine: EngineId; stop: number; hour: number | null };

/** Every action the client may send. */
export type Action =
  | { type: "edit"; edit: NoteEdit }
  | { type: "set_ready"; ready: boolean }
  | { type: "replay_finished" }
  | { type: "continue" };

export interface Car {
  id: number;
  kind: CarKind;
  load: Commodity | null;
  at: { type: "yard"; station: StationId } | { type: "coupled"; engine: EngineId };
}

export interface OrderState {
  id: OrderId;
  commodity: Commodity;
  count: number;
  to: StationId;
  deadline: number;
  delivered: number;
  delivered_at: number | null;
}

export interface OrderOutcome extends OrderState { met: boolean }

export interface MapStation { id: StationId; name: string; factory: boolean }
export interface MapEdge { a: StationId; b: StationId }
export interface MapEngine { id: EngineId; station: StationId; failed: boolean; available_from: number | null }
export interface GameMap {
  stations: MapStation[];
  edges: MapEdge[];
  engines: MapEngine[];
  horizon: number;
  start_hour: number;
}

export interface Announcement {
  type: "engine_failure" | "track_closure" | "late_order";
  message: string;
  [key: string]: unknown;
}

export interface FreightBriefing {
  type: "freight";
  orders: OrderState[];
  cars: Car[];
  stock: { station: StationId; commodities: Commodity[] }[];
  factories: { station: StationId; input: Commodity; output: Commodity; hours: number }[];
  load_hours: number;
  deliver_hours: number;
  announcement: Announcement | null;
}

export interface RoutingBriefing {
  type: "routing";
  travel_times: { a: StationId; b: StationId; hours: number }[];
  restrictions: { a: StationId; b: StationId; from_hour: number; until_hour: number }[];
  capacities: { engine: EngineId; capacity: number }[];
  announcement: Announcement | null;
}

export type Briefing = FreightBriefing | RoutingBriefing;

export interface WorldSnapshot {
  hour: number;
  engines: { id: EngineId; station: StationId; capacity: number; available_from: number; arrived_at: number; failed: boolean }[];
  cars: Car[];
  orders: OrderState[];
}

export type DisruptionNotice =
  | { type: "engine_failure"; engine: EngineId; station: StationId }
  | { type: "track_closure"; a: StationId; b: StationId }
  | { type: "late_order" };

/** A count of cars with one load (`null` = empty), as listed in failure reasons. */
export interface CarCount { load: Commodity | null; count: number }

/** Why a step never ran (`op_abandoned.reason`). */
export type FailReason =
  | { type: "no_matching_cars_in_yard"; kind: CarKind; load: LoadFilter; wanted: number; found: number; yard: CarCount[] }
  | { type: "over_capacity"; wanted: number; carried: number; capacity: number }
  | { type: "no_matching_cars_on_engine"; kind: CarKind; load: LoadFilter; wanted: number; found: number; coupled: CarCount[] }
  | { type: "no_stock_here"; commodity: Commodity }
  | { type: "no_empty_cars_on_engine"; kind: CarKind; wanted: number; found: number }
  | { type: "no_factory_here" }
  | { type: "no_input_cars"; load: Commodity | null; wanted: number; found: number }
  | { type: "wrong_destination"; order: OrderId; to: StationId }
  | { type: "no_order_loads_on_engine"; order: OrderId; commodity: Commodity; coupled: CarCount[] }
  | { type: "order_filled"; order: OrderId }
  | { type: "unknown_order"; order: OrderId };

/** Why an order was missed (`order_missed.why`). */
export type MissReason =
  | { type: "no_car_reached"; commodity: Commodity; to: StationId }
  | { type: "reached_after_deadline"; commodity: Commodity; to: StationId; reached_at: number; deadline: number }
  | { type: "reached_not_delivered"; commodity: Commodity; to: StationId; reached_at: number; delivered: number; count: number }
  | { type: "delivered_after_deadline"; commodity: Commodity; to: StationId; delivered_at: number; deadline: number };

export type EventKind =
  | { type: "arrive"; engine: EngineId; station: StationId; stop: number }
  | { type: "depart"; engine: EngineId; from: StationId; to: StationId; stop: number; arrive_hour: number }
  | { type: "hold"; engine: EngineId; station: StationId; reason: "signal" | "closed" }
  | { type: "op_started"; engine: EngineId; station: StationId; stop: number; index: number; op: Op; finish_hour: number }
  | { type: "op_done"; engine: EngineId; station: StationId; stop: number; index: number; op: Op; cars: number[] }
  | { type: "op_abandoned"; engine: EngineId; station: StationId; stop: number; index: number; op: Op; reason: FailReason }
  | { type: "op_unreached"; engine: EngineId; station: StationId; visit: number; index: number; op: Op }
  | { type: "order_met"; order: OrderId }
  | { type: "order_missed"; order: OrderId; why: MissReason }
  | { type: "disrupted"; what: DisruptionNotice };

export interface GameEvent { hour: number; kind: EventKind }

export interface Trace {
  initial: WorldSnapshot;
  events: GameEvent[];
  end_hour: number;
  orders: OrderOutcome[];
  success: boolean;
}

export interface Divergence {
  hour: number;
  engine: EngineId;
  actual: GameEvent | null;
  expected: GameEvent | null;
}

export interface LastRun {
  number: number;
  act: number;
  success: boolean;
  interrupted_at: number | null;
  actual: Trace;
  expected: Trace;
  divergence: Divergence | null;
  orders: OrderOutcome[];
}

export interface Score {
  scenario: string;
  orders_met: number;
  orders_total: number;
  runs_used: number;
  replanned: boolean;
}

export type Phase = "planning" | "replaying" | "scenario_done" | "finished";
export interface Flag { me: boolean; partner: boolean }

/** The role view delivered as `snapshot.observation`. */
export interface Observation {
  role: Role;
  phase: Phase;
  act: number;
  scenario_index: number;
  scenario_count: number;
  scenario_id: string;
  scenario_title: string;
  scenario_summary: string;
  runs_used: number;
  max_runs: number;
  map: GameMap;
  briefing: Briefing;
  notes: Plan;
  ready: Flag;
  replay_finished: Flag;
  continue: Flag;
  last_run: LastRun | null;
  scores: Score[];
}

export interface Completion { scores: Score[] }

/** Builds the `edit` action wrapping a note edit. */
export function edit(edit: NoteEdit): Action { return { type: "edit", edit }; }

/** Human-readable station name from the shared map, falling back to the id. */
export function stationName(map: GameMap, id: StationId): string {
  return map.stations.find((s) => s.id === id)?.name ?? id;
}

/** Formats an hour as a 12-hour clock time such as "7 am" or "3:50 pm" (hour 24 is "12 am"). */
export function clock(hour: number): string {
  const total = Math.round(hour * 60);
  const h = Math.floor(total / 60) % 24;
  const m = total % 60;
  const h12 = ((h + 11) % 12) + 1;
  return `${h12}${m ? `:${String(m).padStart(2, "0")}` : ""} ${h < 12 ? "am" : "pm"}`;
}

/** Commodities a car kind can carry: boxcars take oranges and bananas, tankers take juice. */
export function commoditiesFor(kind: CarKind): Commodity[] {
  return kind === "boxcar" ? ["oranges", "bananas"] : ["juice"];
}

/** Formats an hour as a clock time with minutes, such as "1:00 am", for film captions and event lists. */
export function clock12(hour: number): string {
  const total = Math.floor((((hour % 24) + 24) % 24) * 60 + 1e-6);
  const h = Math.floor(total / 60), m = total % 60;
  return `${((h + 11) % 12) + 1}:${String(m).padStart(2, "0")} ${h < 12 ? "am" : "pm"}`;
}

/** Plural-aware noun: `plural(2, "boxcar")` is "2 boxcars". */
export function plural(n: number, noun: string): string { return `${n} ${noun}${n === 1 ? "" : "s"}`; }

/** English for a car group, e.g. "1 boxcar", "2 empty tankers", "1 boxcar of oranges". */
export function describeCars(kind: CarKind, load: LoadFilter | Commodity | null, count: number): string {
  const f: LoadFilter = load === null ? { type: "empty" } : typeof load === "string" ? { type: "loaded", commodity: load } : load;
  const noun = plural(count, kind);
  if (f.type === "empty") return noun.replace(" ", " empty ");
  return f.type === "loaded" ? `${noun} of ${f.commodity}` : noun;
}

/** Imperative English description of an op, shared by the notes editor and the narration. */
export function describeOp(op: Op): string {
  switch (op.type) {
    case "couple": return `Couple ${describeCars(op.kind, op.load, op.count)}${op.load.type === "any" ? " (any)" : ""}`;
    case "uncouple": return `Uncouple ${describeCars(op.kind, op.load, op.count)}${op.load.type === "any" ? " (any)" : ""}`;
    case "load": return `Load ${plural(op.count, "car")} with ${op.commodity}`;
    case "process": return `Process ${plural(op.count, "car")} at the factory`;
    case "deliver": return `Deliver ${op.order}`;
  }
}

/** Departure sentence of a stop card: always worded as coming after the stop's steps. */
export function describeDepart(depart: Depart): string {
  switch (depart.type) {
    case "immediately": return "then leaves as soon as its steps are done";
    case "at": return `then leaves at ${clock(depart.hour)} (after its steps)`;
    case "after": return `then leaves ${depart.plus_hours} h after ${depart.engine} (after its steps)`;
  }
}
