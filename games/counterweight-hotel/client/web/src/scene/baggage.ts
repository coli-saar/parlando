import { Vector3 } from "@babylonjs/core/Maths/math.vector";
import type { TransformNode } from "@babylonjs/core/Meshes/transformNode";
import type { HotelSceneState } from "./hotel";

// The final inventory position represents the dispensary locker on the pickup floor.
const medicalRack = 6;

/** Returns a stacked case center in rack-local coordinates. Rack zero travels with the cabin. */
export function rackPosition(rack: number, slot: number) {
  if (rack === medicalRack) return new Vector3(-5.35, 2.275, 1.75);
  if (rack === 0) return new Vector3(-3.13 + (slot % 2) * 1.05, 0.835 + Math.floor(slot / 2) * 0.42, 2.04);
  return new Vector3(-5.35, 0.835 + Math.floor(slot / 2) * 0.42, 1.68 + (slot % 2) * 1.07);
}

type Stop = { position: Vector3; yaw: number; railOffset: number };
type Hoist = (x: number, y: number, z: number, yaw: number, railOffset: number) => void;
const park: Stop = { position: new Vector3(-2.64, 2.17, 2.04), yaw: 0, railOffset: 0 };
type Move = {
  item: TransformNode; floor: number; destination: number; to: Vector3;
  source: number; restack: TransformNode; restackTo: Vector3;
  stops: Stop[]; pickup: number; release: number; segment: number; elapsed: number;
};

/** Plans a reversible carry. Remote porter requests use the clear vertical service well behind the jamb. */
function route(cabin: Vector3, landing: Vector3, floorOffset: number, unloading: boolean) {
  const high = Math.max(2.17, landing.y + 0.24);
  const facing = -Math.PI / 2;
  const stops: Stop[] = [{ position: cabin, yaw: 0, railOffset: 0 },
    { position: new Vector3(cabin.x, high, cabin.z), yaw: 0, railOffset: 0 },
    { position: new Vector3(landing.x, high, 2.04), yaw: facing, railOffset: 0 }];
  if (floorOffset) {
    stops.push({ position: new Vector3(landing.x, high, -0.70), yaw: facing, railOffset: 0 },
      { position: new Vector3(landing.x, high + floorOffset, -0.70), yaw: facing, railOffset: floorOffset });
  }
  stops.push({ position: new Vector3(landing.x, high + floorOffset, landing.z), yaw: facing, railOffset: floorOffset },
    { position: new Vector3(landing.x, landing.y + floorOffset, landing.z), yaw: facing, railOffset: floorOffset });
  // The empty grip travels from the cabin rail to the source and returns home after release.
  const approach = unloading ? [park] : [park, ...stops.slice(2, -1)];
  const departure = unloading ? [...stops.slice(2, -1).reverse(), park] : [park];
  const carry = unloading ? stops : [...stops].reverse();
  if (unloading) approach.push(stops[1]);
  else departure.unshift(stops[1]);
  return { stops: [...approach, ...carry, ...departure], pickup: approach.length, release: approach.length + carry.length - 1 };

}

/** Tracks individual suitcases across acknowledged inventory snapshots and plays moves in order. */
export function createBaggageTransfers(cases: TransformNode[], marked: TransformNode, cabin: TransformNode, shaft: TransformNode, hoist: Hoist) {
  const racks: TransformNode[][] = Array.from({ length: 7 }, () => []);
  const casesByName = new Map(cases.map(item => [item.name, item]));
  const dockedRacks = new Map<TransformNode, number>();
  const moves: Move[] = [];
  let storesFloor = 1;
  let selected: TransformNode | undefined;
  let playbackRate = 1;

  /** Secures a case to its supporting rack so cabin and landing move independently. */
  function dock(item: TransformNode, rack: number, position: Vector3) {
    dockedRacks.set(item, rack);
    item.parent = rack === 0 ? cabin : shaft;
    for (const mesh of item.getChildMeshes()) {
      mesh.isPickable = true;
      mesh.metadata = item.name;
    }
    item.position.copyFrom(position);
    if (rack) item.position.y += (rack === medicalRack ? storesFloor : rack - 1) * 4;
    item.rotation.y = rack === 0 ? 0 : -Math.PI / 2;
  }

  /** Installs the full snapshot on entry, reconnection or a new service. */
  function restore(counts: number[], carriesMarked: boolean) {
    moves.length = 0;
    selected = undefined;
    dockedRacks.clear();
    playbackRate = 1;
    hoist(park.position.x, park.position.y, park.position.z, park.yaw, 0);
    for (const item of cases) item.setEnabled(false);
    marked.setEnabled(false);
    let used = 0;
    counts.forEach((count, rack) => {
      racks[rack].length = 0;
      for (let slot = 0; slot < count; slot++) {
        const item = rack === medicalRack || (rack === 0 && slot === 0 && carriesMarked) ? marked : cases[used++];
        racks[rack].push(item);
        item.setEnabled(true);
        dock(item, rack, rackPosition(rack, slot));
      }
    });
  }

  /** Reports whether the observation restored the racks or queued a single physical trip. */
  function update(state: HotelSceneState, newService: boolean) {
    const hasStores = state.room === "warm" && state.freightStatus != null;
    const waiting = hasStores && state.freightStatus === "awaiting_pickup";
    storesFloor = state.freightFloor ?? 1;
    const counts = [state.loads, ...state.landingLoads, Number(waiting)];
    const deltas = counts.map((count, i) => count - racks[i].length);
    if (!newService && deltas.every(delta => delta === 0)) return;
    const landing = deltas.findIndex((value, i) => i > 0 && value !== 0);
    if (newService || Math.abs(deltas[0]) !== 1 || landing < 1 ||
        deltas[landing] !== -deltas[0] || deltas.filter(Boolean).length !== 2) {
      restore(counts, hasStores && !waiting);
      return "restore";
    }
    const unloading = deltas[0] < 0;
    const source = unloading ? 0 : landing;
    const destination = unloading ? landing : 0;
    const stack = racks[source];
    const selectedSlot = selected ? stack.indexOf(selected) : -1;
    selected = undefined;
    let slot = selectedSlot >= 0 ? selectedSlot : stack.length - 1;
    if (unloading && stack[slot] === marked) slot--;

    const item = stack[slot];
    // Reserve the vacated slot now so any queued trip uses the compacted rack order.
    const restack = stack.pop()!;
    if (item !== restack) stack[slot] = restack;
    const from = rackPosition(source, slot);
    const to = rackPosition(destination, racks[destination].length);
    racks[destination].push(item);
    const offset = ((landing === medicalRack ? storesFloor : landing - 1) - state.floor) * 4;
    const trip = route(unloading ? from : to, unloading ? to : from, offset, unloading);
    moves.push({ item, floor: state.floor, destination, to, source, restack, restackTo: from,
      ...trip, segment: 0, elapsed: 0 });
    playbackRate = Math.max(playbackRate, Math.min(6, moves.length));
    return "transfer";
  }

  /** Moves through the service bay and reports when the current trip has cleared it. */
  function step(seconds: number, cabinFloor: number) {
    const move = moves[0];
    if (!move) return true;
    if (move.floor !== cabinFloor) return false;
    const { item, stops, segment } = move;
    if (segment === move.pickup && move.elapsed === 0) {
      dockedRacks.delete(item);
      for (const mesh of item.getChildMeshes()) {
        mesh.isPickable = false;
        mesh.metadata = undefined;
      }
      item.parent = cabin;
    }
    const start = stops[segment], end = stops[segment + 1];
    // The service hoist covers the inter-floor section faster than a porter carries a case.
    const carrying = segment >= move.pickup && segment < move.release;
    const speed = Math.abs(end.position.y - start.position.y) >= 4 ? 11 : carrying ? 2.9 : 8;
    const duration = Math.max(carrying ? 0.24 : 0.10, Vector3.Distance(start.position, end.position) / speed);
    // Preserve pickup and deposit order while catching up with rapidly acknowledged button presses.
    move.elapsed += seconds * playbackRate;
    const fraction = Math.min(1, move.elapsed / duration);
    const blend = fraction * fraction * (3 - 2 * fraction);
    const position = Vector3.Lerp(start.position, end.position, blend);
    const yaw = start.yaw + (end.yaw - start.yaw) * blend;
    hoist(position.x, position.y, position.z, yaw, start.railOffset + (end.railOffset - start.railOffset) * blend);
    if (carrying) {
      item.position.copyFrom(position);
      item.rotation.y = yaw;
    }
    if (fraction === 1) {
      move.elapsed = 0;
      move.segment++;
      if (move.segment === move.release) {
        dock(item, move.destination, move.to);
        // Compact the visible rack after the hoist clears the selected case.
        if (move.restack !== item) dock(move.restack, move.source, move.restackTo);
      }
      if (move.segment === stops.length - 1) {
        moves.shift();
        if (!moves.length) playbackRate = 1;
        return true;
      }
    }
    return false;
  }

  return {
    update,
    step,
    /** Associates the next acknowledged transfer with the clicked case, when the source matches. */
    selectCase(name: string) { selected = casesByName.get(name); },
    /** Discards an earlier pick before an action issued by the accessible controls. */
    clearSelection() { selected = undefined; },
    /** Resolves a visible case to its physical rack while it is still available there. */
    caseTarget(name: string) {
      const item = casesByName.get(name);
      if (!item) return undefined;
      const rack = dockedRacks.get(item);
      if (rack === undefined || !racks[rack].includes(item)) return null;
      return rack === 0 ? "unload" : `load:${rack - 1}`;
    },
  };
}
