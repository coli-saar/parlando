/**
 * Replay overlays: clock, order sheet, schematic map and caption drawn over the
 * film, plus the progress bar of shots and the "Back to planning" button.
 *
 * Owns presentation only. It receives a `ReplayView` from the player and the
 * static geography for the map; it never sends actions itself (the parent wires
 * `onBack` to ReplayFinished). Styling lives in style.css under `.nf-replay`.
 */
import type { Geography } from "../scene/geography";
import type { GameMap, LastRun } from "../game";
import { clock12, stationName } from "../game";
import { EventsList } from "./EventsList";
import type { ReplayView } from "./player";

/** Small schematic of the railway with the engines. */
function Schematic({ geo, map, view }: { geo: Geography; map: GameMap; view: ReplayView }) {
  const sites = [...geo.stations.values()];
  const xs = sites.map((s) => s.x), zs = sites.map((s) => s.z);
  const x0 = Math.min(...xs) - 120, x1 = Math.max(...xs) + 120, z0 = Math.min(...zs) - 120, z1 = Math.max(...zs) + 120;
  const W = 150, H = 150;
  const sc = Math.min(W / (x1 - x0), H / (z1 - z0));
  const px = (x: number) => 8 + (x - x0) * sc, py = (z: number) => 8 + (z1 - z) * sc;
  return (
    <svg className="nf-schematic" viewBox={`0 0 ${W + 16} ${H + 16}`} role="img" aria-label="Map of the railway with train positions">
      {geo.edges.map((e) => (
        <polyline key={`${e.a}-${e.b}`} className="nf-sch-edge" fill="none"
          points={e.path.points.filter((_, i) => i % 6 === 0).map((p) => `${px(p.x)},${py(p.z)}`).join(" ")} />
      ))}
      {sites.map((s) => (
        <g key={s.id}>
          <circle cx={px(s.x)} cy={py(s.z)} r={3.2} className="nf-sch-station" />
          <text x={px(s.x) + 5} y={py(s.z) - 4} className="nf-sch-label">{stationName(map, s.id)}</text>
        </g>
      ))}
      {view.markers.map((m) => (
        <g key={m.id}>
          <circle cx={px(m.x)} cy={py(m.z)} r={3.6} className="nf-sch-engine" />
          <text x={px(m.x) + 5} y={py(m.z) + 9} className="nf-sch-id">{m.id}</text>
        </g>
      ))}
    </svg>
  );
}

/** The film's HUD. */
export function ReplayOverlay({ view, geo, map, run, onBack }: { view: ReplayView; geo: Geography; map: GameMap; run: LastRun; onBack(): void }) {
  return (
    <div className="nf-replay" aria-live="polite">
      <div className="nf-replay-top">
        <div className={`nf-sheet${view.sheetEmphasis ? " nf-sheet-big" : ""}`} aria-label="Order sheet">
          <h3>Orders</h3>
          {view.orders.map((o) => (
            <div key={o.id} className={`nf-order nf-order-${o.status}`}>
              <span className="nf-tick" aria-hidden="true">{o.status === "met" ? "✓" : o.status === "missed" ? "✗" : "○"}</span>
              <span>{o.id}: {o.count} car{o.count > 1 ? "s" : ""} of {o.commodity} to {stationName(map, o.to)} by {clock12(o.deadline)}</span>
            </div>
          ))}
        </div>
        <Schematic geo={geo} map={map} view={view} />
      </div>
      {view.ended && (
        <div className="nf-end">
          <EventsList run={run} map={map} />
          <button className="nf-primary nf-back" onClick={onBack}>{run.success ? "Continue" : "Back to planning"}</button>
        </div>
      )}
      <div className="nf-replay-bottom">
        {view.work.length > 0 && (
          <div className="nf-work" aria-label="Work in progress">
            {view.work.map((w) => (
              <div key={w.id} className="nf-work-card"><span>{w.label}</span><span className="nf-work-bar"><span style={{ width: `${Math.round(w.progress * 100)}%` }} /></span></div>
            ))}
          </div>
        )}
        <div className="nf-progress" aria-hidden="true"><span style={{ width: `${Math.min(100, (100 * view.filmSeconds) / Math.max(1, view.totalSeconds))}%` }} /></div>
        <div className="nf-caption-row">
          <div className={`nf-caption nf-tone-${view.tone}${view.caption || view.fast ? "" : " nf-caption-empty"}`}>
            {view.fast && <div className="nf-ff" aria-label="Fast-forward">fast-forward ▸▸</div>}
            {view.caption && <div>{view.caption}</div>}
            {view.note && <div className="nf-caption-note">{view.note}</div>}
          </div>
          <div className={`nf-clock${view.fast ? " nf-clock-fast" : ""}`}>{clock12(view.clockHour)}{view.fast && <span aria-hidden="true"> ▸▸</span>}</div>
        </div>
      </div>
    </div>
  );
}
