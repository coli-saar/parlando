type LiftCue =
  | { kind: "travel"; floor: number }
  | { kind: "key"; time: number }
  | { kind: "baggage" };

/** Plays acknowledged lifts, handoffs and local hoist trips in observation order. */
export function createLiftPlayback(keyTravelTime: number) {
  const cues: LiftCue[] = [];
  let floor = 0;
  let origin = floor;
  let elapsedMs = 0;
  let keyTime = 0;

  return {
    /** Installs a recorded service immediately, with the key already in its recorded socket. */
    restore(recordedFloor: number, hasKey: boolean) {
      cues.length = 0;
      floor = origin = recordedFloor;
      elapsedMs = 0;
      keyTime = hasKey ? 0 : keyTravelTime;
    },
    /** Enqueues every landing between two authoritative readings. */
    enqueueTravel(from: number, to: number) {
      const direction = Math.sign(to - from);
      for (let stop = from; stop !== to;) {
        stop += direction;
        cues.push({ kind: "travel", floor: stop });
      }
    },
    /** Holds the cars together while the key crosses into its recorded socket. */
    enqueueKey(hasKey: boolean) {
      cues.push({ kind: "key", time: hasKey ? 0 : keyTravelTime });
    },
    /** Gives the newest hoist trip its place among the authoritative strokes and handoff. */
    enqueueBaggage() {
      cues.push({ kind: "baggage" });
    },
    /** Removes obsolete hoist cues after the baggage animator restores a complete snapshot. */
    clearBaggage() {
      for (let i = cues.length - 1; i >= 0; i--) {
        if (cues[i].kind === "baggage") cues.splice(i, 1);
      }
    },
    /** Advances one cue per render frame and returns the landing reached during this frame. */
    step(frameMs: number, animate: boolean, stepBaggage: (seconds: number, floor: number) => boolean) {
      const cue = cues[0];
      if (!cue) return;
      if (cue.kind === "key") {
        const remaining = cue.time - keyTime;
        keyTime = Math.abs(remaining) <= frameMs / 1000
          ? cue.time
          : keyTime + Math.sign(remaining) * frameMs / 1000;
        if (keyTime === cue.time) cues.shift();
        return;
      }
      if (cue.kind === "baggage") {
        if (stepBaggage(frameMs / 1000, floor)) cues.shift();
        return;
      }

      elapsedMs += frameMs;
      const progress = Math.min(1, elapsedMs / (animate ? 2250 : 600));
      if (progress === 1) {
        floor = origin = cue.floor;
        elapsedMs = 0;
        cues.shift();
        return floor;
      }
      // Without camera motion, keep a stationary audio cue before cutting to the landing.
      const eased = animate ? progress ** 3 * (10 + progress * (-15 + 6 * progress)) : 0;
      floor = origin + (cue.floor - origin) * eased;
    },
    /** Supplies the interpolated local car position in floor units. */
    get floor() { return floor; },
    /** Supplies the accumulated time along the key transfer path. */
    get keyTime() { return keyTime; },
    /** Supplies the next lift landing while a travel cue heads the queue. */
    get destination() { return cues[0]?.kind === "travel" ? cues[0].floor : undefined; },
    /** Reports whether a hoist trip currently holds the cars at their stops. */
    get transferringBaggage() { return cues[0]?.kind === "baggage"; },
    /** Reports whether controls must wait for a queued lift stroke to reach its landing. */
    get travelling() { return cues.some(cue => cue.kind === "travel"); },
    /** Reports whether there is any acknowledged motion left to present. */
    get pending() { return cues.length > 0; },
  };
}
