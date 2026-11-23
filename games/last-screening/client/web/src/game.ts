/** Participant-visible JSON from the authoritative Rust game. */
export type Role = "projectionist" | "stagehand";
export type Level = "moonrise" | "crossing";
export type Phase = "align" | "hide" | "emerge" | "escape";
export type Lantern = 0 | 1;
export type Flat = "window" | "tower";
export type Slide = "moon" | "ship" | "light" | "empty";

export type Action =
  | { type: "aim"; lantern: Lantern; position: number }
  | { type: "load"; lantern: Lantern; slide: Slide }
  | { type: "moveFlat"; flat: Flat; position: number }
  | { type: "exit" };

export interface Screen {
  window: number;
  tower: number;
  moon: number | null;
  light: number | null;
  ship: number | null;
  lit: boolean;
}

export interface Observation {
  /** Present on live observations and omitted by the standalone preview. */
  actions?: Action[];
  role: Role;
  level: Level;
  phase: Phase;
  screen: Screen;
  cue: Partial<Record<Flat, number>>;
  lanterns?: { id: Lantern; aim: number; slide: Slide }[];
  flats?: Record<Flat, number>;
  exited: boolean;
}

export interface Completion {
  escaped: true;
  levelsCompleted: number;
}

export const slides: Record<Slide, string> = {
  moon: "Moon", ship: "Ship", light: "Lamp", empty: "Empty"
};

export const marks = [0, 1, 2, 3, 4, 5, 6];

/** Matches controls to the latest server-supplied action catalogue. */
export function sameAction(a: Action, b: Action): boolean {
  switch (a.type) {
    case "aim":
      return b.type === "aim" && a.lantern === b.lantern && a.position === b.position;
    case "load":
      return b.type === "load" && a.lantern === b.lantern && a.slide === b.slide;
    case "moveFlat":
      return b.type === "moveFlat" && a.flat === b.flat && a.position === b.position;
    case "exit":
      return b.type === "exit";
  }
}

/** Describes the current objective and the assigned operator's cue. */
export function cueFor(observation: Observation): { title: string; instruction: string; mark: string } {
  if (observation.phase === "escape") {
    return {
      title: "The safety curtain",
      instruction: "The curtain has lifted. Both of you through, now.",
      mark: ""
    };
  }
  const { tower, window } = observation.cue;
  const mark = tower !== undefined
    ? `Tower at ${tower + 1}. Tell the stagehand.`
    : window !== undefined ? `Window at ${window + 1}. Tell the projectionist.` : "";
  if (observation.level === "moonrise") {
    return {
      title: "Moonrise at Bell Rock",
      instruction: "Bring the moon into the lighthouse window.",
      mark
    };
  }
  const crossingInstructions = {
    align: "Light the window and show the ship to the left of the tower.",
    hide: "Keep the window lit. Bring the ship behind the tower.",
    emerge: "Keep the window lit. Bring the ship out to the right of the tower."
  };
  return {
    title: "Midnight Crossing",
    instruction: crossingInstructions[observation.phase],
    mark
  };
}

/** Reads visible screen details in plain language for the stage status. */
export function describeScreen(screen: Screen): string {
  const facts = [
    `Window at ${screen.window + 1}, ${screen.lit ? "lit" : "dark"}`,
    `tower at ${screen.tower + 1}`,
    screen.moon === null ? "" : `moon at ${screen.moon + 1}`,
    screen.light === null ? "" : `lamp at ${screen.light + 1}`,
    screen.ship === null ? "" : `ship at ${screen.ship + 1}`
  ];
  return facts.filter(Boolean).join(" · ");
}
