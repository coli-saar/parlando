export type PlayerId = "A" | "B";

export type RoomId =
  | "power"
  | "junction"
  | "oxygen"
  | "valve"
  | "charger"
  | "signal"
  | "airlock"
  | "diagnostics";

export type FuseColor = "blue" | "yellow" | "red";
export type BreakerId = "main" | "aux";
export type ValveId = "A" | "C" | "floodgate";
export type RelayMode = "bypass" | "loop" | "array";
export type BatteryLocation = "charger" | "signal";
export type Direction = "up" | "down" | "left" | "right";

export interface Position {
  x: number;
  y: number;
}

export type GameAction =
  | { type: "moveStep"; player: PlayerId; direction: Direction }
  | { type: "move"; player: PlayerId; room: RoomId }
  | { type: "toggleFuse"; player: PlayerId; color: FuseColor }
  | { type: "toggleBreaker"; player: PlayerId; breaker: BreakerId }
  | { type: "setValve"; player: PlayerId; valve: ValveId; open: boolean }
  | { type: "holdOverride"; player: PlayerId; held: boolean }
  | { type: "togglePlate"; player: PlayerId }
  | { type: "chargeBattery"; player: PlayerId }
  | { type: "moveBattery"; player: PlayerId }
  | { type: "setRelay"; player: PlayerId; mode: RelayMode }
  | { type: "cycleRelay"; player: PlayerId }
  | { type: "runDiagnostic"; player: PlayerId }
  | { type: "launchBeacon"; player: PlayerId }
  | { type: "reset" };

export interface PlayerState {
  room: RoomId;
  position: Position;
  plateHeld: boolean;
}

export interface StationState {
  players: Record<PlayerId, PlayerState>;
  fuses: Record<FuseColor, boolean>;
  breakers: Record<BreakerId, boolean>;
  valves: Record<ValveId, boolean>;
  overrideHeld: boolean;
  battery: {
    location: BatteryLocation;
    charged: boolean;
    spent: boolean;
  };
  relay: RelayMode;
  pressureDrained: boolean;
  oxygenFanTripped: boolean;
  knowledge: Record<PlayerId, string[]>;
  visualEffects: string[];
  log: string[];
  beaconLaunched: boolean;
  moveCount: number;
}

export interface StationObservation extends StationState {
  role?: PlayerId;
  systems?: DerivedSystems;
  privateKnowledge?: string[];
}

export interface ObservationEvent {
  type: string;
  text?: string;
  move_count?: number | null;
  actor?: PlayerId | null;
}

export interface DerivedSystems {
  pumpPowered: boolean;
  chargerFed: boolean;
  coolingRestored: boolean;
  oxygenStable: boolean;
  powerStable: boolean;
  doorAccess: boolean;
  signalRouted: boolean;
  batteryReady: boolean;
  readyToLaunch: boolean;
}

export interface RoomDefinition {
  id: RoomId;
  name: string;
  x: number;
  y: number;
  description: string;
  playerNotes: Partial<Record<PlayerId, string>>;
  exits: RoomId[];
}

export type DeviceKind =
  | "fuse"
  | "breaker"
  | "valve"
  | "bypass"
  | "plate"
  | "charger"
  | "battery"
  | "relay"
  | "diagnostic"
  | "beacon";

export interface DeviceDefinition {
  id: string;
  kind: DeviceKind;
  label: string;
  room: RoomId;
  position: Position;
}

export interface DoorDefinition {
  id: string;
  label: string;
  a: Position;
  b: Position;
  kind: "open" | "pressure";
}
