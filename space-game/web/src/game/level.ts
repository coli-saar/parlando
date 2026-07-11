import type { DeviceDefinition, DoorDefinition, PlayerId, Position, RoomDefinition, RoomId } from "./types";

export const mapWidth = 16;
export const mapHeight = 10;

export const rooms: RoomDefinition[] = [
  {
    id: "power",
    name: "Power Bay",
    x: 0,
    y: 0,
    description: "Fuse rack, main breaker, and a humming emergency bus.",
    playerNotes: {
      A: "Local tags: blue fuse = bus feed, yellow fuse = pump leg, red fuse = reserve.",
      B: "The schematic calls this room Node Lantern, not Power Bay."
    },
    exits: ["junction", "charger"]
  },
  {
    id: "junction",
    name: "Junction",
    x: 1,
    y: 0,
    description: "A pressure door, manual bypass lever, and scuffed floor markings.",
    playerNotes: {
      A: "Bypass placard: HOLD DURING LOAD TRANSFER.",
      B: "Door diagram says the plate carries the latch sensor, not the motor."
    },
    exits: ["power", "oxygen", "diagnostics", "airlock"]
  },
  {
    id: "oxygen",
    name: "Oxygen Nook",
    x: 2,
    y: 0,
    description: "A fan cabinet and duct indicators with delayed pressure lights.",
    playerNotes: {
      A: "The fan trips if the pump line surges through AUX.",
      B: "Duct note: refill pressure after opening any flood path."
    },
    exits: ["junction", "valve"]
  },
  {
    id: "valve",
    name: "Valve Room",
    x: 2,
    y: 1,
    description: "Three hand valves route coolant, water, and cabin pressure.",
    playerNotes: {
      A: "Only the valve letters are visible from your angle.",
      B: "Valve A feeds cabin pressure. Valve C feeds charger water when floodgate is shut."
    },
    exits: ["oxygen", "charger", "signal"]
  },
  {
    id: "charger",
    name: "Battery Charger",
    x: 1,
    y: 1,
    description: "A floor battery dock sits beside a water-cooled charger.",
    playerNotes: {
      A: "Charger wants blue bus, yellow pump, and water return.",
      B: "Moving a charged battery steals the door motor unless bypass is held."
    },
    exits: ["power", "valve", "signal"]
  },
  {
    id: "signal",
    name: "Signal Array",
    x: 2,
    y: 2,
    description: "Relay blades can be set to bypass, loop, or array.",
    playerNotes: {
      A: "ARRAY drinks a full battery pulse when started early.",
      B: "Relay map: bypass keeps doors alive; array points at the beacon."
    },
    exits: ["valve", "charger", "airlock"]
  },
  {
    id: "airlock",
    name: "Airlock",
    x: 1,
    y: 2,
    description: "The evacuation beacon launcher is behind a pressure-linked hatch.",
    playerNotes: {
      A: "Launch checklist: power, air, door, signal.",
      B: "The hatch rejects launch if relay is not locked to ARRAY."
    },
    exits: ["junction", "signal"]
  },
  {
    id: "diagnostics",
    name: "Diagnostics",
    x: 0,
    y: 1,
    description: "Two scratched terminals report symptoms after each test.",
    playerNotes: {
      A: "Terminal A names electrical faults in local rack language.",
      B: "Terminal B names flow faults in schematic language."
    },
    exits: ["junction"]
  }
];

export const roomById = Object.fromEntries(rooms.map((room) => [room.id, room])) as Record<
  RoomId,
  RoomDefinition
>;

export const playerNames: Record<PlayerId, string> = {
  A: "Player A",
  B: "Player B"
};

export const roomRegions: Array<
  RoomDefinition & { left: number; right: number; top: number; bottom: number }
> = [
  { ...roomById.power, left: 1, right: 4, top: 1, bottom: 3 },
  { ...roomById.junction, left: 5, right: 7, top: 1, bottom: 3 },
  { ...roomById.oxygen, left: 8, right: 12, top: 1, bottom: 3 },
  { ...roomById.diagnostics, left: 1, right: 4, top: 5, bottom: 7 },
  { ...roomById.charger, left: 5, right: 7, top: 5, bottom: 7 },
  { ...roomById.valve, left: 8, right: 12, top: 5, bottom: 7 },
  { ...roomById.airlock, left: 5, right: 7, top: 8, bottom: 9 },
  { ...roomById.signal, left: 8, right: 12, top: 8, bottom: 9 }
];

const connectorCells: Array<{ room: RoomId; position: Position }> = [
  { room: "junction", position: { x: 6, y: 4 } },
  { room: "oxygen", position: { x: 10, y: 4 } },
  { room: "diagnostics", position: { x: 2, y: 4 } },
  { room: "charger", position: { x: 6, y: 4 } },
  { room: "valve", position: { x: 10, y: 4 } }
];

export const doors: DoorDefinition[] = [
  { id: "power-junction", label: "Power hatch", a: { x: 4, y: 2 }, b: { x: 5, y: 2 }, kind: "open" },
  { id: "junction-oxygen", label: "Oxygen hatch", a: { x: 7, y: 2 }, b: { x: 8, y: 2 }, kind: "open" },
  { id: "junction-charger", label: "Lower ladder", a: { x: 6, y: 3 }, b: { x: 6, y: 4 }, kind: "open" },
  { id: "charger-junction", label: "Lower ladder", a: { x: 6, y: 4 }, b: { x: 6, y: 5 }, kind: "open" },
  { id: "oxygen-valve", label: "Flow hatch", a: { x: 10, y: 3 }, b: { x: 10, y: 4 }, kind: "open" },
  { id: "valve-oxygen", label: "Flow hatch", a: { x: 10, y: 4 }, b: { x: 10, y: 5 }, kind: "open" },
  { id: "charger-valve", label: "Service door", a: { x: 7, y: 6 }, b: { x: 8, y: 6 }, kind: "open" },
  { id: "charger-airlock", label: "Pressure hatch", a: { x: 6, y: 7 }, b: { x: 6, y: 8 }, kind: "pressure" },
  { id: "valve-signal", label: "Array hatch", a: { x: 10, y: 7 }, b: { x: 10, y: 8 }, kind: "open" },
  { id: "airlock-signal", label: "Beacon hatch", a: { x: 7, y: 8 }, b: { x: 8, y: 8 }, kind: "open" },
  { id: "diagnostics-power", label: "Diagnostic ladder", a: { x: 2, y: 3 }, b: { x: 2, y: 4 }, kind: "open" },
  { id: "diagnostics-ladder", label: "Diagnostic ladder", a: { x: 2, y: 4 }, b: { x: 2, y: 5 }, kind: "open" }
];

export const devices: DeviceDefinition[] = [
  { id: "fuse-blue", kind: "fuse", label: "Blue fuse rack", room: "power", position: { x: 1, y: 1 } },
  { id: "fuse-yellow", kind: "fuse", label: "Yellow fuse rack", room: "power", position: { x: 2, y: 1 } },
  { id: "fuse-red", kind: "fuse", label: "Red fuse rack", room: "power", position: { x: 3, y: 1 } },
  { id: "breaker-main", kind: "breaker", label: "MAIN breaker", room: "power", position: { x: 1, y: 3 } },
  { id: "breaker-aux", kind: "breaker", label: "AUX breaker", room: "power", position: { x: 2, y: 3 } },
  { id: "bypass", kind: "bypass", label: "Manual bypass lever", room: "junction", position: { x: 5, y: 2 } },
  { id: "plate", kind: "plate", label: "Latch floor plate", room: "junction", position: { x: 7, y: 2 } },
  { id: "charger", kind: "charger", label: "Battery charger", room: "charger", position: { x: 5, y: 5 } },
  { id: "battery", kind: "battery", label: "Battery sled", room: "charger", position: { x: 6, y: 6 } },
  { id: "valve-a", kind: "valve", label: "Valve A", room: "valve", position: { x: 8, y: 5 } },
  { id: "valve-c", kind: "valve", label: "Valve C", room: "valve", position: { x: 9, y: 5 } },
  { id: "valve-floodgate", kind: "valve", label: "Floodgate wheel", room: "valve", position: { x: 11, y: 5 } },
  { id: "relay", kind: "relay", label: "Signal relay console", room: "signal", position: { x: 9, y: 8 } },
  { id: "beacon", kind: "beacon", label: "Evacuation beacon", room: "airlock", position: { x: 6, y: 8 } }
];

export function roomAtPosition(position: Position): RoomId | undefined {
  const connector = connectorCells.find((cell) => samePosition(cell.position, position));
  if (connector) return connector.room;

  return roomRegions.find(
    (room) =>
      position.x >= room.left &&
      position.x <= room.right &&
      position.y >= room.top &&
      position.y <= room.bottom
  )?.id;
}

export function isWalkable(position: Position): boolean {
  return Boolean(roomAtPosition(position));
}

export function doorForStep(from: Position, to: Position): DoorDefinition | undefined {
  return doors.find(
    (door) =>
      (samePosition(door.a, from) && samePosition(door.b, to)) ||
      (samePosition(door.b, from) && samePosition(door.a, to))
  );
}

export function isAdjacent(a: Position, b: Position): boolean {
  return Math.abs(a.x - b.x) + Math.abs(a.y - b.y) <= 1;
}

export function nearbyDevices(position: Position): DeviceDefinition[] {
  return devices.filter((device) => isAdjacent(position, device.position));
}

export function devicesAtPosition(position: Position): DeviceDefinition[] {
  return devices.filter((device) => samePosition(device.position, position));
}

export function cells(): Position[] {
  return Array.from({ length: mapWidth * mapHeight }, (_, index) => ({
    x: index % mapWidth,
    y: Math.floor(index / mapWidth)
  }));
}

function samePosition(a: Position, b: Position): boolean {
  return a.x === b.x && a.y === b.y;
}
