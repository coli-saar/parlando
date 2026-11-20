import { Pick } from "@babylonjs/core/Culling/ray.core";
import type { FreeCamera } from "@babylonjs/core/Cameras/freeCamera";
import type { Scene } from "@babylonjs/core/scene";

/** A solid object's footprint in the suite's horizontal world coordinates. */
export interface WalkFootprint {
  minX: number;
  maxX: number;
  minZ: number;
  maxZ: number;
}

const movementKeys = new Map([
  ["a", [-1, 0]], ["arrowleft", [-1, 0]],
  ["d", [1, 0]], ["arrowright", [1, 0]],
  ["w", [0, 1]], ["arrowup", [0, 1]],
  ["s", [0, -1]], ["arrowdown", [0, -1]],
]);
const controlSelector = [
  "a[href]", "button", "input", "select", "textarea", "label", "summary", "form", "nav",
  "[contenteditable]", "[role='button']", "[role='textbox']", "[role='menu']", "[role='dialog']",
].join(", ");
/** Horizontal clearance reserved around the guest at walls and solid objects. */
export const guestRadius = 0.23;
/** Top guest speed in metres per second on foot and on the guided crossing. */
export const walkSpeed = 3.5;

/** Keeps keyboard movement out of UI controls, including controls with nested content. */
function usesKeyboard(target: EventTarget | null) {
  return target instanceof Element && target.closest(controlSelector) !== null;
}

/** Owns local camera movement and drag-to-look for a single suite. */
export function createWalkingControls(
  canvas: HTMLCanvasElement,
  scene: Scene,
  camera: FreeCamera,
  walls: WalkFootprint,
  furniture: WalkFootprint[],
  onPick: (target: string) => void,
  onTargetChange?: (target: string | null) => void,
  onBlockedStep?: (x: number, z: number) => void,
  canPick?: (target: string) => boolean,
  exteriorFloor?: (x: number, z: number) => number | undefined,
) {
  const pressed = new Set<string>();
  let walkX = 0;
  let walkForward = 0;
  let speed = 0;
  let enabled = true;
  let hover: { clientX: number; clientY: number } | undefined;
  let lastHoverTime = -Infinity;
  let currentTarget: string | null = null;
  let drag: { id: number; startX: number; startY: number; x: number; y: number; moved: boolean } | undefined;

  /** Keeps a walk running when the guest focuses the canvas to look. */
  function onFocusIn(event: FocusEvent) {
    if (event.target !== canvas) reset();
  }

  /** Records movement keys only when scene navigation owns the input. */
  function keyDown(event: KeyboardEvent) {
    const key = event.key.toLowerCase();
    if (!enabled || event.defaultPrevented || event.isComposing || event.altKey ||
        event.ctrlKey || event.metaKey || usesKeyboard(event.target) || !movementKeys.has(key)) {
      return;
    }
    event.preventDefault();
    if (!event.repeat) pressed.add(key);
  }

  /** Releases a key regardless of the current focus or modifier keys. */
  function keyUp(event: KeyboardEvent) {
    pressed.delete(event.key.toLowerCase());
  }

  /** Ends an interrupted look gesture and releases the pointer back to the browser. */
  function cancelLook() {
    const id = drag?.id;
    drag = undefined;
    if (id !== undefined && canvas.hasPointerCapture(id)) canvas.releasePointerCapture(id);
  }

  /** Stops motion and look when the browser loses the active view. */
  function reset() {
    pressed.clear();
    speed = 0;
    walkX = 0;
    walkForward = 0;
    cancelLook();
    clearHover();
  }

  /** Picks the nearest interactive mesh at the pointer's canvas-relative coordinates. */
  function targetAt(point: { clientX: number; clientY: number }): string | null {
    const rect = canvas.getBoundingClientRect();
    if (point.clientX < rect.left || point.clientX >= rect.right ||
        point.clientY < rect.top || point.clientY >= rect.bottom) return null;
    // Solid scenery masks objects behind it. The generous transparent hit areas remain usable.
    const hit = Pick(scene, point.clientX - rect.left, point.clientY - rect.top,
      mesh => mesh.isEnabled() && mesh.isVisible && (mesh.isPickable || mesh.visibility === 1));
    const metadata = hit?.pickedMesh?.metadata;
    return typeof metadata === "string" && (canPick?.(metadata) ?? true) ? metadata : null;
  }

  /** Updates the cursor and announces a changed physical target to the surrounding interface. */
  function showTarget(target: string | null) {
    canvas.style.cursor = target ? "pointer" : "grab";
    if (target === currentTarget) return;
    currentTarget = target;
    onTargetChange?.(target);
  }

  /** Clears the retained pointer location when the guest starts looking or leaves the canvas. */
  function clearHover() {
    hover = undefined;
    lastHoverTime = -Infinity;
    showTarget(null);
  }

  /** Starts a primary tap or drag, leaving additional fingers available to touch movement. */
  function pointerDown(event: PointerEvent) {
    if (!enabled || event.button !== 0 || drag) return;
    if (event.pointerType !== "mouse") clearHover();
    canvas.focus({ preventScroll: true });
    drag = {
      id: event.pointerId,
      startX: event.clientX,
      startY: event.clientY,
      x: event.clientX,
      y: event.clientY,
      moved: false,
    };
    canvas.setPointerCapture(event.pointerId);
  }

  /** Rotates through a full turn and caps the camera's vertical angle before either pole. */
  function pointerMove(event: PointerEvent) {
    if (!enabled) return;
    if (!drag) {
      if (event.pointerType === "mouse") {
        hover = { clientX: event.clientX, clientY: event.clientY };
      }
      return;
    }
    if (drag.id !== event.pointerId) return;
    drag.moved ||= Math.hypot(event.clientX - drag.startX, event.clientY - drag.startY) > 5;
    if (!drag.moved) return;
    clearHover();
    canvas.style.cursor = "grabbing";
    // Map a drag across the picture to its angular extent, independent of screen density.
    const radiansPerPixel = 2 * Math.tan(camera.fov / 2) / Math.max(1, canvas.clientHeight);
    camera.rotation.y -= (event.clientX - drag.x) * radiansPerPixel;
    camera.rotation.x = Math.max(-1.35, Math.min(1.35,
      camera.rotation.x - (event.clientY - drag.y) * radiansPerPixel,
    ));
    drag.x = event.clientX;
    drag.y = event.clientY;
  }

  /** Treats a short gesture as an instrument click and completes a drag without clicking. */
  function pointerUp(event: PointerEvent) {
    if (drag?.id !== event.pointerId) return;
    const clicked = !drag.moved &&
      Math.hypot(event.clientX - drag.startX, event.clientY - drag.startY) <= 5;
    cancelLook();
    const target = targetAt(event);
    if (event.pointerType === "mouse") {
      hover = { clientX: event.clientX, clientY: event.clientY };
      showTarget(target);
    }
    if (clicked && target) onPick(target);
  }

  /** Drops only the active look pointer on cancellation or capture loss. */
  function pointerCancel(event: PointerEvent) {
    if (drag?.id !== event.pointerId) return;
    cancelLook();
    clearHover();
  }

  /** Checks a circular guest footprint against the walls and solid furniture. */
  function canStand(x: number, z: number) {
    if (exteriorFloor?.(x, z) !== undefined) return true;
    return x >= walls.minX + guestRadius && x <= walls.maxX - guestRadius &&
      z >= walls.minZ + guestRadius && z <= walls.maxZ - guestRadius &&
      furniture.every((item) => {
        const dx = Math.max(0, item.minX - x, x - item.maxX);
        const dz = Math.max(0, item.minZ - z, z - item.maxZ);
        return dx * dx + dz * dz >= guestRadius * guestRadius;
      });
  }

  window.addEventListener("keydown", keyDown);
  window.addEventListener("keyup", keyUp);
  window.addEventListener("blur", reset);
  document.addEventListener("focusin", onFocusIn);
  document.addEventListener("visibilitychange", reset);
  canvas.addEventListener("pointerdown", pointerDown);
  canvas.addEventListener("pointermove", pointerMove);
  canvas.addEventListener("pointerleave", clearHover);
  canvas.addEventListener("pointerup", pointerUp);
  canvas.addEventListener("pointercancel", pointerCancel);
  canvas.addEventListener("lostpointercapture", pointerCancel);

  return {
    clearHover,
    canStand,
    /** Enables scene navigation and clears held input when another UI takes over. */
    setWalkingEnabled(next: boolean) {
      enabled = next;
      if (!enabled) reset();
    },
    /** Sets analog strafe and forward input in the range [-1, 1]. */
    setWalkVector(x: number, forward: number) {
      if (!enabled) return;
      walkX = Math.max(-1, Math.min(1, x));
      walkForward = Math.max(-1, Math.min(1, forward));
    },
    /** Moves relative to yaw, allowing the guest to slide along solid surfaces. */
    step(elapsedMs: number) {
      if (!enabled || document.hidden) return;
      // Sample once per interval so a moving car also refreshes a stationary pointer.
      const now = performance.now();
      if (!drag && hover && now - lastHoverTime >= 50) {
        lastHoverTime = now;
        showTarget(targetAt(hover));
      }
      let horizontal = 0;
      let vertical = 0;
      for (const key of pressed) {
        const direction = movementKeys.get(key);
        if (!direction) continue;
        horizontal += direction[0];
        vertical += direction[1];
      }
      horizontal = Math.sign(horizontal) + walkX;
      vertical = Math.sign(vertical) + walkForward;
      if (horizontal === 0 && vertical === 0) {
        speed = 0;
        return;
      }
      // Ease into a stride, stop on release, and discard long gaps after a stalled frame.
      const elapsed = Math.min(elapsedMs, 100) / 1000;
      const steps = Math.max(1, Math.ceil(elapsed / 0.02));
      const seconds = elapsed / steps;
      const length = Math.max(1, Math.hypot(horizontal, vertical));
      const sine = Math.sin(camera.rotation.y);
      const cosine = Math.cos(camera.rotation.y);
      // Short collision steps preserve doorway and furniture clearance on slower frames.
      for (let i = 0; i < steps; i++) {
        const previousSpeed = speed;
        speed = Math.min(walkSpeed, speed + seconds * 16);
        const distance = seconds * (previousSpeed + speed) / (2 * length);
        const x = camera.position.x + (horizontal * cosine + vertical * sine) * distance;
        const z = camera.position.z + (vertical * cosine - horizontal * sine) * distance;
        if (canStand(x, camera.position.z)) camera.position.x = x;
        if (canStand(camera.position.x, z)) camera.position.z = z;
        else onBlockedStep?.(camera.position.x, z);
      }
    },
    /** Releases all browser listeners and held input. */
    dispose() {
      reset();
      window.removeEventListener("keydown", keyDown);
      window.removeEventListener("keyup", keyUp);
      window.removeEventListener("blur", reset);
      document.removeEventListener("focusin", onFocusIn);
      document.removeEventListener("visibilitychange", reset);
      canvas.removeEventListener("pointerdown", pointerDown);
      canvas.removeEventListener("pointermove", pointerMove);
      canvas.removeEventListener("pointerleave", clearHover);
      canvas.removeEventListener("pointerup", pointerUp);
      canvas.removeEventListener("pointercancel", pointerCancel);
      canvas.removeEventListener("lostpointercapture", pointerCancel);
    },
  };
}
