/**
 * Game view: lays out the planning screen and the replay for one participant.
 *
 * Owns the single 3D world (created once per map), the planning layout from the
 * design (3D overview and notes bottom-left, briefing and communication widget on
 * the right), the replay layout (the film fills the left area, overlays on top),
 * the scenario-done screen, and the wiring between the view and the replay
 * player. Everything shown comes from `snapshot.observation`; the only local
 * state is presentation (the live replay overlay data, sound mute). Partner
 * `edit` actions are never read here (spec §15.1): they only reach the app as
 * transitions and nothing in this view depends on them.
 */
import { useEffect, useMemo, useRef, useState } from "react";
import { CommunicationWidget } from "@coli-saar/parlando-client/react";
import { Briefing } from "./briefing/Briefing";
import { ScenarioDone } from "./briefing/ScenarioDone";
import { TutorialFilmHint, TutorialHint } from "./briefing/Tutorial";
import { NotesEditor } from "./notes/NotesEditor";
import { ReplayOverlay } from "./replay/ReplayOverlay";
import { ReplayPlayer, type ReplayView } from "./replay/player";
import { Timeline } from "./replay/timeline";
import type { NightFreightWorld } from "./scene/world";
import { stationName, type Action, type Observation, type Trace } from "./game";

/** What the participant runtime hands to the game. */
export interface Snapshot {
  observation: Observation;
  interactionEnabled: boolean;
  transition?: { action: Action } | null;
  act(action: Action): void;
}

const muteKey = "night-freight.muted";

/** Reads the stored sound preference without failing in private windows. */
function readMuted(): boolean {
  try { return localStorage.getItem(muteKey) === "1"; } catch { return false; }
}

/** A trace with no events: the standing world, used to draw the planning overview. */
function standingTrace(o: Observation): Trace {
  return {
    initial: {
      hour: o.map.start_hour,
      engines: o.map.engines.map((e) => ({ id: e.id, station: e.station, capacity: 3, available_from: e.available_from ?? 0, arrived_at: 0, failed: e.failed })),
      cars: o.briefing.type === "freight" ? o.briefing.cars : [],
      orders: []
    },
    events: [], end_hour: o.map.start_hour, orders: [], success: false
  };
}

/** The whole participant-facing game. */
export function GameView({ snapshot }: { snapshot: Snapshot }) {
  const o = snapshot.observation;
  const canvas = useRef<HTMLCanvasElement>(null);
  const stage = useRef<HTMLDivElement>(null);
  const [world, setWorld] = useState<NightFreightWorld | null>(null);
  const [sceneState, setSceneState] = useState<"loading" | "ready" | "unavailable">("loading");
  const [muted, setMuted] = useState(readMuted);
  const [view, setView] = useState<ReplayView | null>(null);
  const player = useRef<ReplayPlayer | null>(null);
  const playing = useRef<string | null>(null);
  const finishedFor = useRef<string | null>(null);
  const actRef = useRef(snapshot.act);
  actRef.current = snapshot.act;
  const mapKey = JSON.stringify([o.map.stations, o.map.edges]);

  // Create the world once per map; it lives for the whole scenario sequence.
  useEffect(() => {
    let created: NightFreightWorld | null = null;
    let cancelled = false;
    void import("./scene/world").then(({ NightFreightWorld }) => {
      if (cancelled || !canvas.current) return;
      try {
        created = new NightFreightWorld(canvas.current, o.map);
        created.sound.setMuted(readMuted());
        setWorld(created);
        setSceneState("ready");
      } catch (error) {
        console.error("3D scene unavailable", error);
        setSceneState("unavailable");
      }
    });
    return () => { cancelled = true; player.current?.stop(); player.current = null; playing.current = null; created?.dispose(); setWorld(null); };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mapKey]);

  const geo = world?.geo ?? null;
  const run = o.last_run;
  const runKey = run ? `${o.scenario_index}:${run.number}:${run.act}` : null;
  const watching = o.phase === "replaying" && run !== null && !o.replay_finished.me;
  const overviewKey = JSON.stringify([o.map.engines, o.briefing.type === "freight" ? o.briefing.cars : null, o.map.start_hour]);

  // Planning overview or film, depending on the phase.
  useEffect(() => {
    if (!world || !geo) return;
    if (watching && run && runKey) {
      if (playing.current === runKey) return;
      player.current?.stop();
      playing.current = runKey;
      world.setHourOverride(null);
      const order = o.map.engines.map((e) => e.id);
      player.current = new ReplayPlayer(world, run, geo, order, (id) => stationName(o.map, id), setView, () => {
        // A successful film acknowledges itself at its end; other films wait for the button.
        if (!player.current?.film.needsAcknowledge && finishedFor.current !== runKey) {
          finishedFor.current = runKey;
          actRef.current({ type: "replay_finished" });
        }
      });
      return;
    }
    if (playing.current !== null || player.current === null) {
      player.current?.stop();
      player.current = null; playing.current = null; setView(null);
      world.setTicker(null);
      world.clearStock();
      world.setHourOverride(9.5);
      world.useOverview();
    }
    const tl = new Timeline(standingTrace(o), geo, o.map.engines.map((e) => e.id));
    world.cut();
    world.showFrame(tl.evaluate(o.map.start_hour), 0);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [world, geo, watching, runKey, overviewKey]);

  useEffect(() => { world?.sound.setMuted(muted); try { localStorage.setItem(muteKey, muted ? "1" : "0"); } catch { /* preference not stored */ } }, [world, muted]);
  useEffect(() => {
    if (!world || !stage.current) return;
    const ro = new ResizeObserver(() => world.engine.resize());
    ro.observe(stage.current);
    return () => ro.disconnect();
  }, [world]);

  const enabled = snapshot.interactionEnabled && o.phase === "planning";
  const tutorial = o.scenario_id === "tutorial";
  const mode = watching ? "replaying" : o.phase === "scenario_done" || o.phase === "finished" ? "done" : "planning";
  const waitingPartnerFilm = o.phase === "replaying" && o.replay_finished.me;

  const header = useMemo(() => (
    <div className="nf-stage-title">
      <span className="nf-title">{o.scenario_title}</span>
      <span className="nf-tag">Scenario {o.scenario_index + 1}/{o.scenario_count}</span>
      <span className="nf-tag">Run {Math.min(o.runs_used + (o.phase === "planning" ? 1 : 0), o.max_runs)} of {o.max_runs}</span>
      {o.act === 2 && <span className="nf-tag nf-tag-alert">Replanning after the disruption</span>}
      <span className="nf-tag nf-role">You are {o.role === "freight" ? "Freight" : "Routing"}</span>
    </div>
  ), [o.scenario_title, o.scenario_index, o.scenario_count, o.runs_used, o.max_runs, o.phase, o.act, o.role]);

  return (
    <main className={`nf-game nf-${mode}`} onPointerDown={() => world?.sound.resume()}>
      <div className="nf-left">
        <div className="nf-stage" ref={stage}>
          <canvas ref={canvas} className="nf-canvas" aria-label="Railway view" />
          {sceneState === "loading" && <div className="nf-stage-note">Loading the railway…</div>}
          {sceneState === "unavailable" && <div className="nf-stage-note">The 3D view is not available in this browser. You can still plan and run.</div>}
          {header}
          {mode === "planning" && sceneState !== "unavailable" && (
            <div className="nf-camera-hint">Drag to turn · Right-drag or Ctrl-drag to move · Scroll to zoom</div>
          )}
          <button type="button" className="nf-mute" aria-pressed={muted} onClick={() => setMuted(!muted)} title="Sound">{muted ? "Sound off" : "Sound on"}</button>
          {mode === "planning" && waitingPartnerFilm && <div className="nf-stage-note nf-banner">Waiting for your partner to finish watching…</div>}
          {watching && view && geo && (
            <>
              <ReplayOverlay view={view} geo={geo} map={o.map} run={run} onBack={() => {
                if (runKey && finishedFor.current !== runKey) { finishedFor.current = runKey; snapshot.act({ type: "replay_finished" }); }
              }} />
              {tutorial && !view.ended && <TutorialFilmHint />}
            </>
          )}
        </div>
        {mode === "planning" && (
          <div className="nf-notes-wrap">
            {tutorial && o.phase === "planning" && <TutorialHint observation={o} />}
            <NotesEditor observation={o} enabled={enabled} act={snapshot.act} />
          </div>
        )}
        {mode === "done" && (
          o.phase === "scenario_done"
            ? <ScenarioDone observation={o} enabled={snapshot.interactionEnabled} act={snapshot.act} />
            : <section className="nf-done"><h2>That was the last scenario</h2><p className="nf-muted">Waiting for the session to finish…</p></section>
        )}
      </div>
      <aside className="nf-right">
        {mode !== "replaying" && <Briefing observation={o} />}
        <div className="nf-dialogue"><CommunicationWidget /></div>
      </aside>
    </main>
  );
}
