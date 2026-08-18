import type { LimbAppearance } from "./visualState";

export interface BudProps {
  x: number;
  y: number;
  rotation: number;
  appearance: LimbAppearance;
}

const SEPAL_FILL: Record<LimbAppearance, string> = {
  dryDark: "#54523b",
  dryLit: "#8a7b3e",
  wetDark: "#54523b",
  flowered: "#f4bcd0"
};

const CENTER_FILL: Record<LimbAppearance, string> = {
  dryDark: "#7c8a6e",
  dryLit: "#e9d979",
  wetDark: "#7c8a6e",
  flowered: "#fff8fa"
};

/** Water swells the bud; sun colours it. Both together replace it with an open blossom. */
export function Bud({ x, y, rotation, appearance }: BudProps) {
  const scale = appearance === "wetDark" ? 1.35 : 1;
  if (appearance === "flowered") {
    return (
      <g transform={`translate(${x},${y})`}>
        <circle r={34} fill="#fff3f6" opacity={0.55} />
        <g fill="#fff8fa" stroke="#f4bcd0" strokeWidth={1}>
          <ellipse cx={0} cy={-11} rx={6} ry={11} />
          <ellipse cx={10.5} cy={-3.4} rx={6} ry={11} transform="rotate(72 10.5 -3.4)" />
          <ellipse cx={6.5} cy={8.9} rx={6} ry={11} transform="rotate(144 6.5 8.9)" />
          <ellipse cx={-6.5} cy={8.9} rx={6} ry={11} transform="rotate(216 -6.5 8.9)" />
          <ellipse cx={-10.5} cy={-3.4} rx={6} ry={11} transform="rotate(288 -10.5 -3.4)" />
        </g>
        <circle r={4.6} fill="#ffd76a" />
      </g>
    );
  }
  return (
    <g transform={`translate(${x},${y}) rotate(${rotation}) scale(${scale})`}>
      <path d="M-5 7 C -9 3 -10 -3 -8 -7 C -5 -3 -4 2 -5 7 Z" fill={SEPAL_FILL[appearance]} />
      <path d="M5 7 C 9 3 10 -3 8 -7 C 5 -3 4 2 5 7 Z" fill={SEPAL_FILL[appearance]} />
      <path
        d="M0 -14 C 5 -9 7 -1 4 6 C 2 10 -2 10 -4 6 C -7 -1 -5 -9 0 -14 Z"
        fill={CENTER_FILL[appearance]}
      />
    </g>
  );
}
