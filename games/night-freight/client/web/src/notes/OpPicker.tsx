/**
 * Cargo-step picker: builds one `Op` of any of the five kinds.
 *
 * Owns the form state (kind, car kind, load filter, commodity, count, order) and
 * produces a wire-format `Op` when the player confirms. The picker only offers
 * choices that can work given what the viewer's role knows for certain:
 * couple/uncouple filters name only commodities the chosen car kind can carry
 * (boxcars take oranges and bananas, tankers take juice) and default to "any";
 * Freight, who sees stock and orders, can load only what the station stocks and
 * deliver only orders destined here; "Process" is offered only at factory
 * stations, which are public. Routing lacks the stock and order facts (its cargo
 * fields are only a belief about Freight's plan), so it keeps those options open.
 */
import { useState } from "react";
import type { CarKind, Commodity, GameMap, LoadFilter, Observation, Op, StationId } from "../game";
import { commoditiesFor } from "../game";

const KINDS: { value: Op["type"]; label: string }[] = [
  { value: "couple", label: "Couple cars" },
  { value: "uncouple", label: "Uncouple cars" },
  { value: "load", label: "Load cars" },
  { value: "process", label: "Process at factory" },
  { value: "deliver", label: "Deliver order" }
];
const LOADABLE: Commodity[] = ["oranges", "bananas"];

/** What the role can know about this stop: which step kinds make sense, and which commodities and orders to offer. */
export interface StopFacts {
  /** Reason a step kind is unavailable here (shown in the menu), or null when it is offered. */
  unavailable: Partial<Record<Op["type"], string>>;
  loadable: Commodity[];
  /** Orders offered for delivery, or null when the role does not know the orders (free entry). */
  orders: string[] | null;
}

/** Derives the offered choices for a stop from the viewer's role, the shared map and the briefing. */
export function stopFacts(o: Observation, station: StationId, map: GameMap = o.map): StopFacts {
  const unavailable: StopFacts["unavailable"] = {};
  if (!map.stations.find((s) => s.id === station)?.factory) unavailable.process = "no factory here";
  const b = o.briefing;
  if (b.type === "freight") {
    const stocked = b.stock.find((s) => s.station === station)?.commodities.filter((c) => LOADABLE.includes(c)) ?? [];
    if (!stocked.length) unavailable.load = "nothing stocked here";
    const orders = b.orders.filter((ord) => ord.to === station).map((ord) => ord.id);
    if (!orders.length) unavailable.deliver = "no order goes here";
    return { unavailable, loadable: stocked, orders };
  }
  return { unavailable, loadable: LOADABLE, orders: null };
}

/** Form for a new cargo step at one stop; calls `onAdd` with the finished op. */
export function OpPicker({ facts, onAdd }: { facts: StopFacts; onAdd(op: Op): void }) {
  const firstOk = KINDS.find((k) => !facts.unavailable[k.value])?.value ?? "couple";
  const [type, setType] = useState<Op["type"]>(firstOk);
  const [carKind, setCarKind] = useState<CarKind>("boxcar");
  const [filter, setFilter] = useState<"any" | "empty" | Commodity>("any");
  const [commodity, setCommodity] = useState<Commodity>(facts.loadable[0] ?? "oranges");
  const [count, setCount] = useState(1);
  const [order, setOrder] = useState(facts.orders?.[0] ?? "O1");
  const carries = commoditiesFor(carKind);
  const effective: "any" | "empty" | Commodity = filter === "any" || filter === "empty" || carries.includes(filter) ? filter : "any";
  const loadOptions = facts.loadable.length ? facts.loadable : LOADABLE;

  /** The wire load filter for the current form values. */
  const loadFilter = (): LoadFilter => (effective === "any" ? { type: "any" } : effective === "empty" ? { type: "empty" } : { type: "loaded", commodity: effective });

  /** Assembles the wire op from the current form values. */
  function build(): Op {
    switch (type) {
      case "couple":
      case "uncouple": return { type, kind: carKind, load: loadFilter(), count };
      case "load": return { type, commodity: loadOptions.includes(commodity) ? commodity : loadOptions[0], count };
      case "process": return { type, count };
      case "deliver": return { type, order };
    }
  }
  return (
    <form className="nf-form" onSubmit={(e) => { e.preventDefault(); onAdd(build()); }}>
      <label>Step
        <select value={type} onChange={(e) => setType(e.target.value as Op["type"])}>
          {KINDS.map((k) => (
            <option key={k.value} value={k.value} disabled={!!facts.unavailable[k.value]}>
              {k.label}{facts.unavailable[k.value] ? ` (${facts.unavailable[k.value]})` : ""}
            </option>
          ))}
        </select>
      </label>
      {(type === "couple" || type === "uncouple") && (
        <>
          <label>Car
            <select value={carKind} onChange={(e) => setCarKind(e.target.value as CarKind)}>
              <option value="boxcar">boxcar</option><option value="tanker">tanker</option>
            </select>
          </label>
          <label>Which cars
            <select value={effective} onChange={(e) => setFilter(e.target.value as "any" | "empty" | Commodity)}>
              <option value="any">any</option>
              <option value="empty">empty</option>
              {carries.map((c) => <option key={c} value={c}>loaded with {c}</option>)}
            </select>
          </label>
        </>
      )}
      {type === "load" && (
        <label>Commodity
          <select value={loadOptions.includes(commodity) ? commodity : loadOptions[0]} onChange={(e) => setCommodity(e.target.value as Commodity)}>
            {loadOptions.map((c) => <option key={c} value={c}>{c}</option>)}
          </select>
        </label>
      )}
      {type !== "deliver" && (
        <label>Cars
          <select value={count} onChange={(e) => setCount(Number(e.target.value))}>
            {[1, 2, 3, 4, 5, 6].map((n) => <option key={n} value={n}>{n}</option>)}
          </select>
        </label>
      )}
      {type === "deliver" && (
        <label>Order
          {facts.orders ? (
            <select value={order} onChange={(e) => setOrder(e.target.value)}>
              {facts.orders.map((o) => <option key={o} value={o}>{o}</option>)}
            </select>
          ) : (
            <input value={order} maxLength={4} onChange={(e) => setOrder(e.target.value.toUpperCase())} />
          )}
        </label>
      )}
      <button type="submit" className="nf-primary" disabled={!!facts.unavailable[type]}>Add step</button>
    </form>
  );
}
