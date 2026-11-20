import type { Action, BridgePose } from "./game";

const sampleIntervalMs = 250;
const positionResolution = 20;
const headingResolution = 20;

/**
 * Samples a frame-by-frame pose source for the existing participant action channel.
 * A disabled sender forgets its last sample so the current pose is sent again
 * when interaction resumes after a reconnect. Cabin reentry sends a clear once.
 */
export function createBridgePosePublisher(send: (action: Action) => void) {
  let previous: BridgePose | null | undefined;
  let lastSent = -Infinity;

  /** Publishes a changed sample once enough time has elapsed since the last send. */
  return (pose: BridgePose | null, enabled: boolean, now = performance.now()) => {
    if (!enabled) {
      previous = undefined;
      lastSent = -Infinity;
      return;
    }
    if (pose === null) {
      if (previous !== null) send({ type: "bridge_pose", pose: null });
      previous = null;
      return;
    }
    if (now - lastSent < sampleIntervalMs) return;
    const yaw = Math.atan2(Math.sin(pose.yaw), Math.cos(pose.yaw));
    const sample = {
      x: Math.round(pose.x * positionResolution) / positionResolution,
      z: Math.round(pose.z * positionResolution) / positionResolution,
      yaw: Math.max(-Math.PI, Math.min(Math.PI, Math.round(yaw * headingResolution) / headingResolution)),
    };
    if (previous?.x === sample.x && previous.z === sample.z && previous.yaw === sample.yaw) return;
    previous = sample;
    lastSent = now;
    send({ type: "bridge_pose", pose: sample });
  };
}
