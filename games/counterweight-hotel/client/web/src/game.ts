/** JSON values sent by the hotel server to a single participant. */
export type Wing = "east" | "west";
/** The experiment chooses one evacuation route for the entire session. */
export type Route = "escape" | "rescue_supplies";
/** Stage of the marked East consignment and the two load manifests. */
export type FreightStatus = "awaiting_pickup" | "aboard" | "ready";
export type Seal = "stag" | "rose" | "key";
/** Coordinates on the shared crossing with a heading in radians from positive Z. */
export interface BridgePose {
  x: number;
  z: number;
  yaw: number;
}

export type Action =
  | { type: "load" | "unload" | "release" | "reach_refuge" | "reset_lock" | "collect_rescue_case" | "pass" | "step_onto_landing" | "board_lift" }
  | { type: "load_rack"; floor: number }
  | { type: "exit"; seal: Seal }
  | { type: "bridge_pose"; pose: BridgePose | null };

export interface Observation {
  wing: Wing;
  route: Route;
  route_title: string;
  floor: number;
  partner_floor: number;
  /** Next local landing under the current loads, or null at balance or an end stop. */
  release_floor: number | null;
  freight_status: FreightStatus | null;
  /** Position of the stores dock on the viewer's scale. */
  freight_floor: number | null;
  cargo: number;
  partner_cargo: number;
  /** Cases on this wing's fixed landing racks, indexed from Ground to Floor 4. */
  landing_loads: number[];
  /** Suitcases visible on the five fixed shelves beside the other car. */
  partner_landing_loads: number[];
  /** Source and destination racks selected by the baggage service for the next transfer. */
  load_floor: number;
  unload_floor: number;
  /** An open gate secures the cars until both guests have reached the refuge. */
  gate_open: boolean;
  partner_gate_open: boolean;
  /** A tripped inspection lock can only be reset from the brake pedestal. */
  lock_tripped: boolean;
  partner_lock_tripped: boolean;
  /** A guest on either terrace holds both lifts at their current landings. */
  on_landing: boolean;
  partner_on_landing: boolean;
  /** The wire field names the lift whose fixed brake is unlocked by the brass key. */
  handle: Wing;
  handed_over: boolean;
  exited: boolean;
  partner_exited: boolean;
  /** Latest public crossing pose. Historical observations may omit it. */
  partner_bridge_pose?: BridgePose | null;
  partner_destination: number;
  partner_seal: Seal;
  partner_delivery: number | null;
  actions: Action[];
}

export type Completion = { escaped: true; route: Route };

/** Converts the coupled cars' local floor numbers to the East stores address printed on both waybills. */
export function eastStoresFloor(wing: Wing, localFloor: number): number {
  return wing === "east" ? localFloor : 4 - localFloor;
}
