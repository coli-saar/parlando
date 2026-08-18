export interface IceShardsProps {
  x: number;
  y: number;
  /** Angle, in degrees, that the shard cluster's long axis runs along. */
  rotation: number;
  /** How far along that axis the shards spread. */
  length: number;
  count: number;
}

/**
 * A frozen root's ice sheath. Previously just a scatter of flat diamonds, which read as
 * decoration rather than "frozen" — real complaint. Now: a soft blurred cold halo hugging the
 * root (uses the `icy` blur filter defined in RootView's `<defs>`), plus six-pointed frost-star
 * crystals instead of plain diamonds, each with a thin dark outline for contrast against both
 * the pale iced bark and the dark earth, and a slow staggered shimmer.
 */
export function IceShards({ x, y, rotation, length, count }: IceShardsProps) {
  const shards = Array.from({ length: count }, (_, i) => {
    const t = count === 1 ? 0.5 : i / (count - 1);
    const along = t * length;
    const size = 6.5 - 1.5 * Math.abs(t - 0.5);
    const jitter = (i % 2 === 0 ? -1 : 1) * 3;
    return (
      <g
        key={i}
        className="glints"
        style={{ animationDelay: `${(i * 0.7) % 4}s` }}
        transform={`translate(${along},${jitter}) rotate(${20 + i * 11})`}
      >
        <path
          d={`M0 ${-size} L0 ${size} M${-size} 0 L${size} 0 M${-size * 0.7} ${-size * 0.7} L${size * 0.7} ${size * 0.7} M${-size * 0.7} ${size * 0.7} L${size * 0.7} ${-size * 0.7}`}
          stroke="#eef9ff"
          strokeWidth={1.3}
          strokeLinecap="round"
        />
        <circle r={size * 0.28} fill="#f4fdff" />
      </g>
    );
  });
  return (
    <g transform={`translate(${x},${y}) rotate(${rotation})`}>
      <ellipse
        cx={length / 2}
        cy={0}
        rx={length / 2 + 10}
        ry={16}
        fill="#a8dcf2"
        opacity={0.22}
        filter="url(#icy)"
      />
      {shards}
    </g>
  );
}
