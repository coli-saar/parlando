import { useState } from "react";
import { GameView } from "./GameView";
import type { Action, Level, Observation, Screen, Slide } from "./game";

const params = new URLSearchParams(window.location.search);
const stagehand = params.get("role") === "stagehand";
const level: Level = params.get("level") === "crossing" ? "crossing" : "moonrise";
const escape = params.get("phase") === "escape";
const crossing = level === "crossing";

const initial: Observation = {
  role: stagehand ? "stagehand" : "projectionist",
  level,
  phase: escape ? "escape" : crossing ? "hide" : "align",
  screen: { window: crossing ? 4 : 2, tower: 3, moon: crossing ? null : 1, light: crossing ? 4 : null, ship: crossing ? 1 : null, lit: crossing },
  cue: stagehand ? { window: 4 } : { tower: 3 },
  flats: stagehand ? { window: crossing ? 4 : 2, tower: 3 } : undefined,
  lanterns: stagehand ? undefined : [
    { id: 0, aim: 1, slide: crossing ? "ship" : "moon" },
    { id: 1, aim: 4, slide: crossing ? "light" : "empty" }
  ],
  exited: false
};

/** Reflects equipment adjustments in the local preview image. */
function showAdjustment(before: Observation, action: Action): Observation {
  if (action.type === "exit") return { ...before, exited: true };
  if (action.type === "moveFlat" && before.flats) {
    const flats = { ...before.flats, [action.flat]: action.position };
    const screen = { ...before.screen, ...flats };
    return { ...before, flats, screen: { ...screen, lit: screen.light === flats.window } };
  }
  if ((action.type === "load" || action.type === "aim") && before.lanterns) {
    const lanterns = before.lanterns.map((lantern) => lantern.id !== action.lantern ? lantern : action.type === "load"
      ? { ...lantern, slide: action.slide } : { ...lantern, aim: action.position });
    const visible = (slide: Slide) => lanterns.find((lantern) => lantern.slide === slide)?.aim ?? null;
    const ship = visible("ship");
    const screen: Screen = {
      ...before.screen, moon: visible("moon"), light: visible("light"),
      ship: ship === before.screen.tower ? null : ship,
      lit: visible("light") === before.screen.window
    };
    return { ...before, lanterns, screen };
  }
  return before;
}

/** Renders developer-only fixture views without contacting a participant server. */
export default function Preview() {
  const [observation, setObservation] = useState(initial);
  return (
    <>
      <GameView snapshot={{
        observation,
        interactionEnabled: !observation.exited,
        act: (action) => setObservation((before) => showAdjustment(before, action))
      }} />
      <nav className="preview-angles" aria-label="Preview scenes">
        <a href="?preview&role=projectionist">Projectionist · Moonrise</a>
        <a href="?preview&role=stagehand">Stagehand · Moonrise</a>
        <a href="?preview&role=projectionist&level=crossing">Projectionist · Crossing</a>
        <a href="?preview&role=stagehand&level=crossing">Stagehand · Crossing</a>
        <a href="?preview&role=stagehand&level=crossing&phase=escape">Safety curtain</a>
      </nav>
    </>
  );
}
