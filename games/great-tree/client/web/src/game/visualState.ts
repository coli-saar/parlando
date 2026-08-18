import type { LimbView, RootView } from "./types";

export type LimbAppearance = "dryDark" | "dryLit" | "wetDark" | "flowered";

/**
 * Sun and water compose independently (spec §5): sun sets colour, water sets bark/size. Both
 * present is the only way to reach "flowered" — the server's `flowered` derivation and this one
 * must agree, but are computed independently on each side per Parlando's architecture (the
 * server never sends a precomputed "flowered" flag).
 */
export function limbAppearance(view: LimbView): LimbAppearance {
  if (view.sun && view.water) return "flowered";
  if (view.sun) return "dryLit";
  if (view.water) return "wetDark";
  return "dryDark";
}

export type RootAppearance = "iced" | "thawed" | "running";

export function rootAppearance(view: RootView): RootAppearance {
  if (view.running) return "running";
  if (view.thawed) return "thawed";
  return "iced";
}
