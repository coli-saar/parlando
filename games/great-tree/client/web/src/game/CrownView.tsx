import { LIMB_GEOMETRY } from "./geometry";
import { limbAppearance } from "./visualState";
import type { GateState } from "./Gate";
import { Gate } from "./Gate";
import { Bud } from "./Bud";
import { offsetPathX, reversePath } from "./svgPath";
import type { LimbId, LimbView } from "./types";
import { ALL_LIMB_IDS } from "./types";

export interface CrownViewProps {
  limbs: LimbView[];
  onSetSun: (limb: LimbId, lit: boolean) => void;
}

/**
 * Renders Crown's entire half of the tree from `limbs` alone: sky, sun, trunk, the five
 * always-visible trunk channels, and the five limbs. Ported from mockup-great-tree.html's
 * Crown panel — every background/trunk element there has a counterpart here, not just the
 * limbs, because "the tree" is the trunk and sky as much as it is the branches.
 */
export function CrownView({ limbs, onSetSun }: CrownViewProps) {
  const byId = new Map(limbs.map((limb) => [limb.id, limb]));

  return (
    <svg viewBox="0 0 400 300" role="img" aria-label="The crown of the great tree">
      <defs>
        <linearGradient id="sky" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stopColor="#5fa8cc" />
          <stop offset="34%" stopColor="#9fcdd8" />
          <stop offset="64%" stopColor="#f4cf96" />
          <stop offset="100%" stopColor="#f0a866" />
        </linearGradient>
        <radialGradient id="sunGlow" cx="0.5" cy="0.5" r="0.5">
          <stop offset="0%" stopColor="#fffdf2" stopOpacity={1} />
          <stop offset="26%" stopColor="#ffeeb6" stopOpacity={0.7} />
          <stop offset="62%" stopColor="#ffd98a" stopOpacity={0.24} />
          <stop offset="100%" stopColor="#ffc46a" stopOpacity={0} />
        </radialGradient>
        <linearGradient id="trunkBark" x1="0" y1="0" x2="1" y2="0">
          <stop offset="0%" stopColor="#2c1d0d" />
          <stop offset="45%" stopColor="#5e3f24" />
          <stop offset="75%" stopColor="#7c5732" />
          <stop offset="100%" stopColor="#331f0e" />
        </linearGradient>
        <linearGradient id="dryBark" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stopColor="#e4d9c3" />
          <stop offset="55%" stopColor="#c2b192" />
          <stop offset="100%" stopColor="#9c8b70" />
        </linearGradient>
        <linearGradient id="wetBark" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stopColor="#4a2c12" />
          <stop offset="52%" stopColor="#2a1707" />
          <stop offset="100%" stopColor="#100801" />
        </linearGradient>
        <radialGradient id="flowerGlow" cx="0.5" cy="0.5" r="0.5">
          <stop offset="0%" stopColor="#fff6f8" stopOpacity={0.95} />
          <stop offset="100%" stopColor="#ffd0dd" stopOpacity={0} />
        </radialGradient>
        <filter id="far" x="-30%" y="-30%" width="160%" height="160%">
          <feGaussianBlur stdDeviation="9" />
        </filter>
        <filter id="mid" x="-30%" y="-30%" width="160%" height="160%">
          <feGaussianBlur stdDeviation="5" />
        </filter>
      </defs>

      <rect width={400} height={300} fill="url(#sky)" />
      <circle cx={336} cy={76} r={112} fill="url(#sunGlow)" />
      <circle cx={336} cy={76} r={22} fill="#fffcee" />

      <g opacity={0.2} fill="#6f9b86" filter="url(#far)">
        <ellipse cx={90} cy={292} rx={100} ry={20} />
        <ellipse cx={270} cy={298} rx={118} ry={18} />
      </g>
      <g opacity={0.26} filter="url(#mid)">
        <ellipse cx={120} cy={24} rx={66} ry={22} fill="#2a5540" />
        <ellipse cx={330} cy={252} rx={56} ry={22} fill="#2c5a42" />
      </g>

      <g className="sways">
        <path
          d="M10 300 C 18 232 22 176 32 118 C 38 80 44 40 48 -6 L118 -6 C 114 44 108 84 102 124 C 94 182 90 240 94 300 Z"
          fill="url(#trunkBark)"
        />
        <g stroke="#1f1206" strokeWidth={1.6} fill="none" opacity={0.5}>
          <path d="M36 300 C 42 236 48 178 58 108 C 62 74 66 40 68 -4" />
          <path d="M62 300 C 66 240 70 184 78 120 C 82 86 86 46 88 -4" />
        </g>

        {ALL_LIMB_IDS.map((id) => {
          const view = byId.get(id);
          if (!view) return null;
          const geometry = LIMB_GEOMETRY[id];
          // Sun and water are drawn as two parallel lines, offset a few units to either side of
          // the channel's centre, rather than stacked on the same path — stacked, the top one
          // (water) fully hid the bottom one (sun) whenever a limb had both. Sun keeps to the
          // near (uphill) side, water to the far (downhill) side, consistently across every
          // channel, so the two roles are always the same lane once you've seen one.
          const sunPath = offsetPathX(geometry.channelPath, -2.5);
          const waterPath = offsetPathX(geometry.channelPath, 2.5);
          return (
            <g key={`channel-${id}`}>
              <path
                d={geometry.channelPath}
                stroke="#241708"
                strokeWidth={9}
                fill="none"
                strokeLinecap="round"
                opacity={0.4}
              />
              {view.sun && (
                <>
                  <path
                    d={sunPath}
                    stroke="#ffdf8a"
                    strokeWidth={3.4}
                    fill="none"
                    strokeLinecap="round"
                    strokeDasharray="14 30"
                    className="flow"
                  />
                  <path
                    d={sunPath}
                    stroke="#fffbe6"
                    strokeWidth={1.3}
                    fill="none"
                    strokeLinecap="round"
                    strokeDasharray="6 38"
                    className="flow flow-b"
                    opacity={0.85}
                  />
                </>
              )}
              {view.water && (
                // Now safe to animate as live flow: under the current mechanics, water can only
                // ever be true while its root is currently running (removing sun clears both in
                // the same step — see mechanics.rs), so this is never a stale/latched signal.
                <>
                  <path
                    d={reversePath(waterPath)}
                    stroke="#7fe0ff"
                    strokeWidth={3.4}
                    fill="none"
                    strokeLinecap="round"
                    strokeDasharray="14 30"
                    className="flow"
                  />
                  <path
                    d={reversePath(waterPath)}
                    stroke="#ffffff"
                    strokeWidth={1.3}
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

        {ALL_LIMB_IDS.map((id) => {
          const view = byId.get(id);
          if (!view) return null;
          const geometry = LIMB_GEOMETRY[id];
          const appearance = limbAppearance(view);
          const wet = appearance === "wetDark" || appearance === "flowered";
          const gateState: GateState = view.sun ? "open" : "closed";
          return (
            <g key={id}>
              {geometry.barkPaths.map((bark, i) => (
                <path
                  key={i}
                  d={bark.d}
                  stroke={wet ? "url(#wetBark)" : "url(#dryBark)"}
                  strokeWidth={bark.width}
                  fill="none"
                  strokeLinecap="round"
                />
              ))}
              <ellipse
                cx={geometry.leaf.x}
                cy={geometry.leaf.y}
                rx={15}
                ry={8.5}
                fill="#7f9070"
                opacity={0.85}
                transform={`rotate(${geometry.leaf.rotation} ${geometry.leaf.x} ${geometry.leaf.y})`}
              />
              <Bud x={geometry.bud.x} y={geometry.bud.y} rotation={geometry.bud.rotation} appearance={appearance} />
              <Gate
                x={geometry.gate.x}
                y={geometry.gate.y}
                rotation={geometry.gate.rotation}
                state={gateState}
                palette="gold"
                onClick={() => onSetSun(id, !view.sun)}
              />
            </g>
          );
        })}
      </g>

      <g fill="#fff3cd">
        <circle className="mote" cx={152} cy={212} r={1.8} style={{ animationDelay: "0s" }} />
        <circle className="mote" cx={252} cy={184} r={1.5} style={{ animationDelay: "2.4s" }} />
        <circle className="mote" cx={320} cy={142} r={1.9} style={{ animationDelay: "4.8s" }} />
      </g>
    </svg>
  );
}
