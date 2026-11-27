/**
 * Tutorial guidance for the `tutorial` scenario: short overlays that point at the
 * notes editor, the Ready button and the replay.
 *
 * Owns the wording per role and stage. The stage derives from the view alone
 * (runs used, readiness, whether the notes contain anything yet), so it survives
 * reloads. The component also toggles a pulse class on the element it points at.
 */
import { useEffect } from "react";
import type { Observation } from "../game";

/** Stage of the tutorial for planning screens. */
function stage(o: Observation): "write" | "ready" | "retry" {
  if (o.runs_used > 0) return "retry";
  const engines = Object.values(o.notes.engines);
  const touched = engines.some((stops) => stops.length > 1 || stops.some((s) => s.ops.length > 0 || s.depart.type !== "immediately"));
  return touched && !o.ready.me ? "ready" : "write";
}

/** Planning hint bar for the tutorial. */
export function TutorialHint({ observation: o }: { observation: Observation }) {
  const s = stage(o);
  useEffect(() => {
    const target = document.querySelector(s === "ready" ? '[data-tutorial="ready"]' : '[data-tutorial="notes"]');
    target?.classList.add("nf-pulse");
    return () => target?.classList.remove("nf-pulse");
  }, [s]);
  const freight = o.role === "freight";
  const text = s === "write"
    ? (freight
      ? "Tutorial. You are Freight: you know the order and that a boxcar waits in the Corning yard. Routing decides where and when the trains go; you say what happens at each place. The solid steps are yours: at Corning couple the boxcar and load it with oranges (in any order), then add the place Dansville with \"+ place\" and deliver the order there. Press \"+ step\" on a place to add a cargo step."
      : "Tutorial. You are Routing: you know the travel times. The route and the departure times are yours. Use \"+\" to add the next station; a departure is the earliest time the train may leave, and it always leaves after its cargo steps. Freight knows how long loading takes, so ask."
    )
    : s === "ready"
      ? "Compare notes by talking. When you both believe the plan is right, press Ready to run. Editing your notes withdraws your readiness."
      : "The run was only a test of your joint plan. Read the Events list and the caption \"Your notes\", which says what your own notes expected. Talk about where they differed, fix your notes and run again.";
  return <aside className="nf-tutorial" role="note"><strong>Tutorial</strong> {text.replace(/^Tutorial\.\s*/, "")}</aside>;
}

/** One-line hint shown on top of the tutorial film. */
export function TutorialFilmHint() {
  return <aside className="nf-tutorial nf-tutorial-film" role="note"><strong>Tutorial</strong> The film shows what really happened. Where your own notes expected something different, a caption “Your notes: …” says so and the Events list shows both.</aside>;
}
