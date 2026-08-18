import { ROOT_GEOMETRY } from "./geometry";
import { rootAppearance } from "./visualState";
import type { GateState } from "./Gate";
import { Gate } from "./Gate";
import { IceShards } from "./IceShards";
import { offsetPathX, reversePath } from "./svgPath";
import type { RootId, RootView as RootViewData } from "./types";
import { ALL_ROOT_IDS } from "./types";

export interface RootViewProps {
  roots: RootViewData[];
  onSetFlow: (root: RootId, open: boolean) => void;
}

/**
 * Renders Root's entire half of the tree from `roots` alone: earth, soil, the flared trunk
 * base, the five always-visible trunk channels, and the five roots. Ported from
 * mockup-great-tree.html's Root panel.
 */
export function RootView({ roots, onSetFlow }: RootViewProps) {
  const byId = new Map(roots.map((root) => [root.id, root]));

  return (
    <svg viewBox="0 0 400 300" role="img" aria-label="The roots of the great tree">
      <defs>
        <radialGradient id="earth" cx="0.46" cy="0.06" r="1.1">
          <stop offset="0%" stopColor="#5c4128" />
          <stop offset="45%" stopColor="#33220f" />
          <stop offset="100%" stopColor="#1c130c" />
        </radialGradient>
        <radialGradient id="mineralGlow" cx="0.5" cy="0.5" r="0.5">
          <stop offset="0%" stopColor="#bfe8ff" stopOpacity={0.5} />
          <stop offset="100%" stopColor="#bfe8ff" stopOpacity={0} />
        </radialGradient>
        <linearGradient id="rootSkin" x1="0" y1="0" x2="1" y2="1">
          <stop offset="0%" stopColor="#a3743f" />
          <stop offset="52%" stopColor="#704926" />
          <stop offset="100%" stopColor="#3c2513" />
        </linearGradient>
        <linearGradient id="rootCold" x1="0" y1="0" x2="1" y2="1">
          <stop offset="0%" stopColor="#9fb2c2" />
          <stop offset="55%" stopColor="#6d7b8c" />
          <stop offset="100%" stopColor="#3d4552" />
        </linearGradient>
        <radialGradient id="baseGlow" cx="0.5" cy="0.5" r="0.5">
          <stop offset="0%" stopColor="#ffdba0" stopOpacity={0.5} />
          <stop offset="100%" stopColor="#ffb877" stopOpacity={0} />
        </radialGradient>
        <filter id="icy" x="-40%" y="-40%" width="180%" height="180%">
          <feGaussianBlur stdDeviation="6" />
        </filter>
      </defs>

      <rect width={400} height={300} fill="url(#earth)" />

      <g opacity={0.4}>
        <path d="M0 104 C 90 92 200 114 400 96 L400 130 C 210 146 90 124 0 138 Z" fill="#3a2716" />
        <path d="M0 200 C 120 188 260 210 400 192 L400 228 C 250 246 120 224 0 238 Z" fill="#31200e" />
      </g>
      <g fill="#4c3d2c" opacity={0.5}>
        <ellipse cx={40} cy={152} rx={17} ry={9} transform="rotate(-18 40 152)" />
        <ellipse cx={372} cy={130} rx={13} ry={8} transform="rotate(24 372 130)" />
        <ellipse cx={126} cy={272} rx={19} ry={10} transform="rotate(9 126 272)" />
        <ellipse cx={336} cy={292} rx={15} ry={8} transform="rotate(-14 336 292)" />
        <ellipse cx={20} cy={248} rx={14} ry={8} transform="rotate(30 20 248)" />
      </g>

      {/* Distant, unreachable veins of mineral light — pure atmosphere, so the earth reads as
          alive rather than just dark and empty. */}
      <g fill="none" stroke="#7fc9e8" strokeLinecap="round" opacity={0.16}>
        <path d="M0 190 C 60 178 100 200 150 186" strokeWidth={2.5} />
        <path d="M330 250 C 370 240 390 258 400 250" strokeWidth={2.5} />
      </g>
      <circle cx={70} cy={186} r={18} fill="url(#mineralGlow)" opacity={0.5} />
      <circle cx={365} cy={252} r={14} fill="url(#mineralGlow)" opacity={0.5} />

      <path
        d="M100 -8 L312 -8 C 310 18 304 36 292 46 C 264 64 226 68 200 68 C 170 68 136 62 116 46 C 104 36 100 16 100 -8 Z"
        fill="#5a3d1e"
      />
      <path
        d="M116 -8 L296 -8 C 294 14 288 30 278 38 C 252 52 224 56 200 56 C 174 56 146 52 128 38 C 118 30 116 12 116 -8 Z"
        fill="#6d4a26"
        opacity={0.75}
      />
      <ellipse cx={200} cy={34} rx={76} ry={20} fill="url(#baseGlow)" />

      {ALL_ROOT_IDS.map((id) => {
        const view = byId.get(id);
        if (!view) return null;
        const geometry = ROOT_GEOMETRY[id];
        // Same technique as CrownView: sun (down) and water (up) get their own parallel lane
        // instead of sharing one path, where the top layer used to hide the bottom one whenever
        // a root was both thawed and running.
        const thawPath = offsetPathX(geometry.channelPath, -2.5);
        const runPath = offsetPathX(geometry.channelPath, 2.5);
        return (
          <g key={`channel-${id}`}>
            <path
              d={geometry.channelPath}
              stroke="#4a3814"
              strokeWidth={9}
              fill="none"
              strokeLinecap="round"
              opacity={0.4}
            />
            {view.thawed && (
              <>
                <path
                  d={thawPath}
                  stroke="#ffe28f"
                  strokeWidth={3.2}
                  fill="none"
                  strokeLinecap="round"
                  strokeDasharray="14 30"
                  className="flow"
                />
                <path
                  d={thawPath}
                  stroke="#fffbe6"
                  strokeWidth={1.2}
                  fill="none"
                  strokeLinecap="round"
                  strokeDasharray="6 38"
                  className="flow flow-b"
                  opacity={0.85}
                />
              </>
            )}
            {view.running && (
              <>
                <path
                  d={reversePath(runPath)}
                  stroke="#c4f3ff"
                  strokeWidth={3.4}
                  fill="none"
                  strokeLinecap="round"
                  strokeDasharray="14 30"
                  className="flow"
                />
                <path
                  d={reversePath(runPath)}
                  stroke="#ffffff"
                  strokeWidth={1.2}
                  fill="none"
                  strokeLinecap="round"
                  strokeDasharray="6 38"
                  className="flow flow-b"
                  opacity={0.85}
                />
              </>
            )}
          </g>
        );
      })}

      {ALL_ROOT_IDS.map((id) => {
        const view = byId.get(id);
        if (!view) return null;
        const geometry = ROOT_GEOMETRY[id];
        const appearance = rootAppearance(view);
        const bodyColor = appearance === "iced" ? "url(#rootCold)" : "url(#rootSkin)";
        const gateState: GateState =
          appearance === "running" ? "open" : appearance === "thawed" ? "openable" : "frozen";
        return (
          <g key={id}>
            {id === "swollen" && <ellipse cx={304} cy={152} rx={28} ry={24} fill={bodyColor} />}
            {geometry.bodyPaths.map((body, i) => (
              <path
                key={i}
                d={body.d}
                stroke={bodyColor}
                strokeWidth={body.width}
                fill="none"
                strokeLinecap="round"
              />
            ))}
            {appearance === "running" &&
              // Body paths are authored collar-to-tip (down); reverse so the rising animation
              // actually travels tip-to-collar (up), matching every other "water rises" cue in
              // this game rather than silently flowing the wrong way.
              geometry.bodyPaths.map((body, i) => (
                <path
                  key={`water-${i}`}
                  d={reversePath(body.d)}
                  stroke="#c4f3ff"
                  strokeWidth={Math.max(4, body.width * 0.3)}
                  fill="none"
                  strokeLinecap="round"
                  strokeDasharray="16 32"
                  className="rising-water"
                />
              ))}
            {appearance === "iced" && (
              <IceShards
                x={geometry.gate.x}
                y={geometry.gate.y}
                rotation={geometry.iceShards.rotation}
                length={geometry.iceShards.length}
                count={geometry.iceShards.count}
              />
            )}
            <Gate
              x={geometry.gate.x}
              y={geometry.gate.y}
              rotation={geometry.gate.rotation}
              state={gateState}
              palette="cyan"
              onClick={() => onSetFlow(id, !view.running)}
            />
          </g>
        );
      })}

      <g fill="#bfe8ff">
        <circle className="mote" cx={110} cy={220} r={1.6} style={{ animationDelay: "1.2s" }} />
        <circle className="mote" cx={250} cy={180} r={1.3} style={{ animationDelay: "3.6s" }} />
        <circle className="mote" cx={340} cy={210} r={1.7} style={{ animationDelay: "5.4s" }} />
      </g>
    </svg>
  );
}
