import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import type { GameSnapshot } from "@coli-saar/parlando-client/react";
import { cueFor, describeScreen, marks, sameAction, slides, type Action, type Flat, type Observation, type Slide } from "./game";
import type { mountCinema } from "./scene/cinema";

/** Game inputs shared by the participant renderer and the standalone preview. */
export type Snapshot = Pick<GameSnapshot<Observation, Action>, "observation" | "interactionEnabled" | "act">;
type Station = "home" | "lantern-0" | "lantern-1" | Flat | "exit";

const stationNames: Record<Station, string> = {
  home: "Room",
  "lantern-0": "Lantern A",
  "lantern-1": "Lantern B",
  window: "Window flat",
  tower: "Tower flat",
  exit: "Safety curtain"
};

/** Mounts one theater for the assigned role and synchronizes it with the SDK's snapshots. */
export function GameView({ snapshot }: { snapshot: Snapshot }) {
  const { observation, interactionEnabled } = snapshot;
  const projectionist = observation.role === "projectionist";
  const escape = observation.phase === "escape";
  const stations: Station[] = projectionist ? ["lantern-0", "lantern-1"] : ["window", "tower"];
  const [selectedStation, setSelectedStation] = useState<Station>("home");
  const [sceneStatus, setSceneStatus] = useState<"loading" | "ready" | "error">("loading");
  const scene = useRef<ReturnType<typeof mountCinema> | null>(null);
  const canvas = useRef<HTMLCanvasElement | null>(null);
  const currentView = useRef({ observation, station: "home" as Station });
  const station = escape ? "exit" : selectedStation === "exit" ? "home" : selectedStation;
  const cue = cueFor(observation);

  useEffect(() => {
    const element = canvas.current;
    if (!element) return;
    setSceneStatus("loading");
    let cancelled = false;
    let mounted: ReturnType<typeof mountCinema> | undefined;
    void import("./scene/cinema")
      .then(({ mountCinema: createScene }) => {
        if (cancelled) return;
        mounted = createScene(element, observation.role, (selection) => {
          if (Object.hasOwn(stationNames, selection)) setSelectedStation(selection as Station);
        });
        const { observation: latest, station: focus } = currentView.current;
        mounted.update(latest);
        mounted.focus(focus);
        scene.current = mounted;
        setSceneStatus("ready");
      })
      .catch((error: unknown) => {
        if (cancelled) return;
        console.error("The cinema renderer could not start.", error);
        mounted?.dispose();
        mounted = undefined;
        setSceneStatus("error");
      });
    return () => {
      cancelled = true;
      scene.current = null;
      mounted?.dispose();
    };
  }, [observation.role]);

  useEffect(() => {
    currentView.current = { observation, station: currentView.current.station };
    scene.current?.update(observation);
  }, [observation]);

  useEffect(() => {
    currentView.current = { observation: currentView.current.observation, station };
    scene.current?.focus(station);
  }, [station]);

  const lantern = observation.lanterns?.find(({ id }) => station === `lantern-${id}`);
  const flat = station === "window" || station === "tower" ? station : null;
  const position = lantern?.aim ?? (flat && observation.flats ? observation.flats[flat] : undefined);

  /** Checks participant input against the live connection and current action catalogue. */
  function allowed(action: Action): boolean {
    return interactionEnabled && (observation.actions?.some((candidate) => sameAction(action, candidate)) ?? true);
  }

  /** Submits an action only while it is valid for this participant snapshot. */
  function submit(action: Action) {
    if (allowed(action)) snapshot.act(action);
  }

  /** Produces the role-specific action behind a numbered position control. */
  function positionAction(mark: number): Action | undefined {
    if (lantern) return { type: "aim", lantern: lantern.id, position: mark };
    if (flat) return { type: "moveFlat", flat, position: mark };
    return undefined;
  }

  /** Handles room and position shortcuts while focus is inside the game surface. */
  function handleKey(event: KeyboardEvent<HTMLElement>) {
    if (event.ctrlKey || event.altKey || event.metaKey) return;
    if (event.target instanceof HTMLElement && event.target.closest("input, select, textarea, [contenteditable]")) return;
    if (event.key === "0" && !escape) setSelectedStation("home");
    else if (event.key === "1" && !escape) setSelectedStation(stations[0]);
    else if (event.key === "2" && !escape) setSelectedStation(stations[1]);
    else if (event.key.toLowerCase() === "e" && escape) submit({ type: "exit" });
    else if (event.target === canvas.current && position !== undefined && ["ArrowLeft", "ArrowRight"].includes(event.key)) {
      const next = position + (event.key === "ArrowRight" ? 1 : -1);
      const action = positionAction(next);
      if (marks.includes(next) && action) submit(action);
    } else return;
    event.preventDefault();
  }

  return (
    <main className="screening" onKeyDown={handleKey}>
      <header className="masthead">
        <h1>The Last Screening</h1>
        <span className="role-plate">{projectionist ? "Projectionist" : "Stagehand"}</span>
      </header>
      <section className="cinema-frame" aria-label="Cinema">
        <canvas
          ref={canvas}
          tabIndex={sceneStatus === "error" ? -1 : 0}
          aria-label="Interactive theater. Use 1 and 2 to choose equipment, 0 to return to the room, and left or right arrow to move selected equipment."
        />
        {sceneStatus === "loading" && <p className="scene-loading" role="status">Opening the theater…</p>}
        {sceneStatus === "error" && <p className="scene-fallback" role="status">The 3D theater could not start. Use the cue and equipment controls below to play.</p>}
      </section>
      <div className="control-deck">
        <div className="cue-card" aria-live="polite">
          <p className="eyebrow">{escape ? "Final cue" : observation.level === "moonrise" ? "Cue 01" : "Cue 02"}</p>
          <h2>{cue.title}</h2>
          <p>{cue.instruction}</p>
          {cue.mark && <p className="cue-mark-note">{cue.mark}</p>}
          {!escape && <CuePicture observation={observation} />}
          <p className={sceneStatus === "ready" ? "sr-only" : "screen-reading"} aria-label="Visible on screen">{describeScreen(observation.screen)}</p>
        </div>
        <div className="equipment">
          {escape ? (
            <div className="exit-control">
              <button className="exit-button" type="button" disabled={!allowed({ type: "exit" })} onClick={() => submit({ type: "exit" })}>
                {observation.exited ? "Through the curtain" : "Step through the curtain"}
                {!observation.exited && <kbd>E</kbd>}
              </button>
              {observation.exited && <p role="status">Waiting for your partner.</p>}
            </div>
          ) : (
            <>
              <nav aria-label="Equipment" className="station-controls">
                {(["home", ...stations] as Station[]).map((id, shortcut) => (
                  <button key={id} type="button" aria-pressed={station === id} onClick={() => setSelectedStation(id)}>
                    {stationNames[id]} <kbd>{shortcut}</kbd>
                  </button>
                ))}
              </nav>
              {position === undefined ? (
                <p className="station-hint">
                  {projectionist ? "Select a lantern to aim it or change its slide." : "Select a flat to slide it along the stage."}
                </p>
              ) : (
                <div className="station-adjustments">
                  <div className="control-row" role="group" aria-label={projectionist ? "Aim" : "Track position"}>
                    <span className="row-label">{projectionist ? "Aim" : "Track"}</span>
                    <div className="detents">
                      {marks.map((mark) => {
                        const action = positionAction(mark);
                        return (
                          <button key={mark} type="button" aria-label={`Position ${mark + 1}`}
                            aria-pressed={position === mark}
                            disabled={!action || !allowed(action)}
                            onClick={() => action && submit(action)}
                          >{mark + 1}</button>
                        );
                      })}
                    </div>
                  </div>
                  {lantern && (
                    <div className="control-row" role="group" aria-label="Slides">
                      <span className="row-label">Slide</span>
                      <div className="slide-controls">
                        {(Object.keys(slides) as Slide[]).map((slide) => {
                          const action: Action = { type: "load", lantern: lantern.id, slide };
                          return (
                            <button key={slide} type="button" aria-pressed={slide === lantern.slide}
                              disabled={!allowed(action)} onClick={() => submit(action)}
                            >{slides[slide]}</button>
                          );
                        })}
                      </div>
                    </div>
                  )}
                </div>
              )}
            </>
          )}
        </div>
      </div>
    </main>
  );
}


/** Illustrates the intended arrangement at the cue's numbered stage marks. */
function CuePicture({ observation }: { observation: Observation }) {
  const crossing = observation.level === "crossing";
  const { tower, window } = observation.cue;
  return (
    <div className="cue-picture" aria-hidden="true">
      <div className="cue-figures">
        {crossing && <span className="cue-ship-path" />}
        {tower !== undefined && <span className="cue-tower" style={{ gridColumn: tower + 1 }} />}
        {window !== undefined && (
          <span className={crossing ? "cue-window lit" : "cue-window"} style={{ gridColumn: window + 1 }}><span /></span>
        )}
      </div>
      <div className="cue-scale">
        {marks.map((mark) => (
          <span key={mark} className={mark === observation.cue.window || (crossing && mark === observation.cue.tower) ? "cue-mark" : ""}>{mark + 1}</span>
        ))}
      </div>
    </div>
  );
}
