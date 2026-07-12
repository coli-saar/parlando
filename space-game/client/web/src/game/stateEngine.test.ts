import { describe, expect, it } from "vitest";
import { availableActions, deriveSystems, runStateEngine, initialState } from "./stateEngine";
import type { GameAction, StationState } from "./types";

function apply(actions: GameAction[], start: StationState = initialState): StationState {
  return actions.reduce(runStateEngine, start);
}

function holdingPlate(state: StationState): StationState {
  return {
    ...state,
    players: {
      ...state.players,
      B: {
        ...state.players.B,
        position: { x: 7, y: 2 },
        room: "junction",
        plateHeld: true
      }
    }
  };
}

const basicPower: GameAction[] = [
  { type: "toggleFuse", player: "A", color: "blue" },
  { type: "toggleBreaker", player: "A", breaker: "main" },
  { type: "toggleFuse", player: "A", color: "yellow" }
];

const chargeableStation: GameAction[] = [
  ...basicPower,
  { type: "setValve", player: "B", valve: "C", open: true },
  { type: "setValve", player: "B", valve: "A", open: true }
];

describe("space station puzzle state engine", () => {
  it("only offers actions for the device the player is standing on", () => {
    expect(availableActions(initialState, "A")).toEqual([]);

    const onYellowFuse = runStateEngine(initialState, {
      type: "moveStep",
      player: "A",
      direction: "up"
    });
    expect(availableActions(onYellowFuse, "A")).toEqual([
      { type: "toggleFuse", player: "A", color: "yellow" }
    ]);
  });

  it("holds the latch plate only while a player is physically standing on it", () => {
    const onPlate = apply([
      { type: "moveStep", player: "B", direction: "right" },
      { type: "moveStep", player: "B", direction: "up" },
      { type: "moveStep", player: "B", direction: "up" },
      { type: "moveStep", player: "B", direction: "up" },
      { type: "moveStep", player: "B", direction: "up" },
      { type: "moveStep", player: "B", direction: "left" },
      { type: "moveStep", player: "B", direction: "left" },
      { type: "moveStep", player: "B", direction: "left" }
    ]);

    expect(onPlate.players.B.position).toEqual({ x: 7, y: 2 });
    expect(onPlate.players.B.plateHeld).toBe(true);
    expect(availableActions(onPlate, "B")).toEqual([]);

    const offPlate = runStateEngine(onPlate, {
      type: "moveStep",
      player: "B",
      direction: "right"
    });
    expect(offPlate.players.B.plateHeld).toBe(false);
  });

  it("does not bind hint actions to diagnostic terminals", () => {
    const inDiagnostics = apply([
      { type: "moveStep", player: "A", direction: "down" },
      { type: "moveStep", player: "A", direction: "down" },
      { type: "moveStep", player: "A", direction: "down" }
    ]);

    expect(inDiagnostics.players.A.room).toBe("diagnostics");
    expect(availableActions(inDiagnostics, "A")).toEqual([]);
  });

  it("offers one relay action that cycles through the relay modes", () => {
    const atRelay = {
      ...initialState,
      players: {
        ...initialState.players,
        B: {
          ...initialState.players.B,
          room: "signal" as const,
          position: { x: 9, y: 8 }
        }
      }
    };

    expect(availableActions(atRelay, "B")).toEqual([{ type: "cycleRelay", player: "B" }]);
    const loop = runStateEngine(atRelay, { type: "cycleRelay", player: "B" });
    expect(loop.relay).toBe("loop");
    expect(loop.visualEffects).toContain("device:relay");
    const array = runStateEngine(loop, { type: "cycleRelay", player: "B" });
    expect(array.relay).toBe("array");
  });

  it("requires players to cross room borders through door tiles", () => {
    const atWallSeam = apply([
      { type: "moveStep", player: "A", direction: "up" },
      { type: "moveStep", player: "A", direction: "right" },
      { type: "moveStep", player: "A", direction: "right" }
    ]);
    expect(atWallSeam.players.A.position).toEqual({ x: 4, y: 1 });

    const blocked = runStateEngine(atWallSeam, {
      type: "moveStep",
      player: "A",
      direction: "right"
    });
    expect(blocked.players.A.position).toEqual({ x: 4, y: 1 });
    expect(blocked.log[0]).toContain("no door");

    const throughDoor = apply([
      { type: "moveStep", player: "A", direction: "down" },
      { type: "moveStep", player: "A", direction: "right" }
    ], blocked);
    expect(throughDoor.players.A.room).toBe("junction");
    expect(throughDoor.players.A.position).toEqual({ x: 5, y: 2 });
  });

  it("reveals causal knowledge after a failed diagnostic-style action", () => {
    const state = apply([{ type: "chargeBattery", player: "A" }]);

    expect(state.knowledge.A).toContain(
      "Bus feed dark: blue fuse and MAIN breaker must both be live."
    );
    expect(state.log[0]).toContain("refuses the cycle");
  });

  it("lets actions undo a desired oxygen outcome and then recover it", () => {
    const oxygenReady = apply(chargeableStation);
    expect(deriveSystems(oxygenReady).oxygenStable).toBe(true);

    const drained = runStateEngine(oxygenReady, {
      type: "setValve",
      player: "B",
      valve: "floodgate",
      open: true
    });
    expect(deriveSystems(drained).oxygenStable).toBe(false);
    expect(drained.knowledge.B).toContain(
      "Opening the floodgate restores coolant but drains cabin pressure first."
    );

    const recovered = runStateEngine(drained, {
      type: "setValve",
      player: "B",
      valve: "A",
      open: true
    });
    expect(deriveSystems(recovered).oxygenStable).toBe(true);
  });

  it("makes breaker and fuse order matter for the oxygen fan", () => {
    const tripped = apply([
      { type: "toggleBreaker", player: "A", breaker: "aux" },
      { type: "toggleFuse", player: "A", color: "yellow" },
      { type: "toggleFuse", player: "A", color: "blue" },
      { type: "toggleBreaker", player: "A", breaker: "main" }
    ]);

    expect(tripped.oxygenFanTripped).toBe(true);
    expect(deriveSystems(tripped).oxygenStable).toBe(false);
    expect(tripped.knowledge.A).toContain(
      "Yellow pump surge trips the oxygen fan when AUX is already live."
    );
  });

  it("requires the held bypass while transferring the charged battery for door access", () => {
    const charged = apply([
      ...chargeableStation,
      { type: "chargeBattery", player: "A" }
    ]);
    expect(deriveSystems(charged).batteryReady).toBe(true);

    const movedWithoutBypass = apply(
      [{ type: "moveBattery", player: "A" }],
      charged
    );
    expect(deriveSystems(movedWithoutBypass).doorAccess).toBe(false);

    const movedWithBypass = apply(
      [{ type: "holdOverride", player: "A", held: true }, { type: "moveBattery", player: "A" }],
      charged
    );
    expect(deriveSystems(holdingPlate(movedWithBypass)).doorAccess).toBe(true);
  });

  it("requires all final systems to be stable at launch time", () => {
    const almostReady = apply([
      ...chargeableStation,
      { type: "chargeBattery", player: "A" },
      { type: "holdOverride", player: "A", held: true },
      { type: "moveBattery", player: "A" },
      { type: "setValve", player: "B", valve: "C", open: false },
      { type: "setValve", player: "B", valve: "floodgate", open: true },
      { type: "setValve", player: "B", valve: "A", open: true }
    ]);

    const almostReadyWithPlate = holdingPlate(almostReady);
    expect(deriveSystems(almostReadyWithPlate).readyToLaunch).toBe(false);

    const earlyLaunch = runStateEngine(almostReadyWithPlate, { type: "launchBeacon", player: "B" });
    expect(earlyLaunch.beaconLaunched).toBe(false);
    expect(earlyLaunch.battery.spent).toBe(true);

    const ready = apply([
      ...chargeableStation,
      { type: "chargeBattery", player: "A" },
      { type: "holdOverride", player: "A", held: true },
      { type: "moveBattery", player: "A" },
      { type: "setValve", player: "B", valve: "C", open: false },
      { type: "setValve", player: "B", valve: "floodgate", open: true },
      { type: "setValve", player: "B", valve: "A", open: true },
      { type: "setRelay", player: "B", mode: "array" }
    ]);

    const readyWithPlate = holdingPlate(ready);
    expect(deriveSystems(readyWithPlate).readyToLaunch).toBe(true);
    const launched = runStateEngine(readyWithPlate, { type: "launchBeacon", player: "B" });
    expect(launched.beaconLaunched).toBe(true);
  });
});
