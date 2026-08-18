/** Mirrors Rust `LimbId` (`games/great-tree/server/src/game/ids.rs`). */
export type LimbId = "spire" | "hook" | "fork" | "cradle" | "nub";
export const ALL_LIMB_IDS: LimbId[] = ["spire", "hook", "fork", "cradle", "nub"];

/** Mirrors Rust `RootId`. */
export type RootId = "hand" | "knot" | "tip" | "swollen" | "deep";
export const ALL_ROOT_IDS: RootId[] = ["hand", "knot", "tip", "swollen", "deep"];

/** Mirrors Rust `LimbView`. */
export interface LimbView {
  id: LimbId;
  sun: boolean;
  water: boolean;
}

/** Mirrors Rust `RootView`. */
export interface RootView {
  id: RootId;
  thawed: boolean;
  running: boolean;
}

/** Mirrors Rust `GreatTreeObservation`, tagged on `role`. */
export type GreatTreeObservation =
  | { role: "crown"; limbs: LimbView[] }
  | { role: "root"; roots: RootView[] };

/** Mirrors Rust `GreatTreeAction`, tagged on `type`. */
export type GreatTreeAction =
  | { type: "setSun"; limb: LimbId; lit: boolean }
  | { type: "setFlow"; root: RootId; open: boolean };

/** Mirrors Rust `GreatTreeCompletion`. */
export interface GreatTreeCompletion {
  floweredLimbs: LimbId[];
}
