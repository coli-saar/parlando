export type GateState = "closed" | "openable" | "frozen" | "open";

export interface GateProps {
  x: number;
  y: number;
  rotation: number;
  state: GateState;
  /** "gold" for Crown's sun gates, "cyan" for Root's water gates. */
  palette?: "gold" | "cyan";
  onClick: () => void;
}

const FILL: Record<GateState, Record<"gold" | "cyan", string>> = {
  closed: { gold: "#4a3f30", cyan: "#3a4048" },
  openable: { gold: "#8a6a3a", cyan: "#3f6a78" },
  // Frozen looks the same regardless of palette — ice is ice, not a dim version of gold or cyan.
  frozen: { gold: "#b7d2e0", cyan: "#b7d2e0" },
  open: { gold: "#ffdf8a", cyan: "#9ff0ff" }
};

/**
 * The single clickable widget in the whole game (spec §6): a small rounded "seed" node, not a
 * ring-and-pupil (which read as an eye — a real complaint from playtesting). Filled solid and
 * colour-coded by state; frozen additionally gets a frost crack mark and cold halo so "you
 * cannot open this yet" reads as a distinct fact, not just a dimmer version of "closed".
 * Rendered as an SVG <g role="button"> so it is both this shape and reachable as a real
 * interactive element for keyboard/AT users.
 *
 * Frozen is disabled outright rather than dispatching and letting the server reject it with
 * `root_frozen`: unlike "is this move a good idea", "is this root currently thawed" is not a
 * guess the client is making — `state` was computed from the same observation field the server
 * itself checks, so refusing the click here tells the player the same fact the server would
 * have, immediately and silently, instead of via a rejected action they can't see.
 */
export function Gate({ x, y, rotation, state, palette = "gold", onClick }: GateProps) {
  const fill = FILL[state][palette];
  const glowing = state === "open";
  const frozen = state === "frozen";
  return (
    <g
      role="button"
      aria-disabled={frozen || undefined}
      tabIndex={frozen ? -1 : 0}
      aria-label="gate"
      transform={`translate(${x},${y}) rotate(${rotation})`}
      onClick={frozen ? undefined : onClick}
      onKeyDown={
        frozen
          ? undefined
          : (event) => {
              if (event.key === "Enter" || event.key === " ") onClick();
            }
      }
      style={{ cursor: frozen ? "default" : "pointer", outline: frozen ? "none" : undefined }}
    >
      {glowing && (
        <circle r={14} fill={palette === "gold" ? "#fff2c0" : "#bff0ff"} opacity={0.32} />
      )}
      {frozen && <circle r={13} fill="#dff2fb" opacity={0.24} />}
      <rect
        x={-6}
        y={-9}
        width={12}
        height={18}
        rx={6}
        ry={6}
        fill={fill}
        stroke={frozen ? "#eef9ff" : "#241a10"}
        strokeWidth={frozen ? 1.6 : 1.2}
        className={glowing ? "breathes" : undefined}
      />
      {frozen && (
        <g stroke="#f4fdff" strokeWidth={1.4} strokeLinecap="round" opacity={0.9}>
          <path d="M-3.5 -5 L3.5 5 M3.5 -5 L-3.5 5" />
        </g>
      )}
    </g>
  );
}
