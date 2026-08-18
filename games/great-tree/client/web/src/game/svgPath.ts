/**
 * Reverses a single-segment cubic-bezier path (`M x y C x y, x y, x y`), by reversing its point
 * order. Channels are authored in the direction sun travels (down/toward-collar); water in the
 * same channel travels the opposite way, so its overlay reuses the same "advance forward along
 * the path" CSS animation on this reversed path instead of needing a second hand-authored path.
 */
export function reversePath(d: string): string {
  const points = d.match(/-?\d+(\.\d+)?/g)?.map(Number) ?? [];
  const pairs: [number, number][] = [];
  for (let i = 0; i < points.length; i += 2) {
    pairs.push([points[i], points[i + 1]]);
  }
  const reversed = [...pairs].reverse();
  const [start, ...rest] = reversed;
  return `M${start[0]} ${start[1]} C ${rest.map(([x, y]) => `${x} ${y}`).join(" ")}`;
}

/**
 * Shifts every x-coordinate of a single-segment cubic-bezier path by `dx`, leaving y unchanged.
 * Used to draw two channel overlays (sun down, water up) as visually separate parallel lines
 * instead of stacked exactly on top of each other, where the top one hides the bottom one.
 */
export function offsetPathX(d: string, dx: number): string {
  const numbers = d.match(/-?\d+(\.\d+)?/g)?.map(Number) ?? [];
  const shifted = numbers.map((n, i) => (i % 2 === 0 ? n + dx : n));
  const [mx, my, ...rest] = shifted;
  const cPairs: string[] = [];
  for (let i = 0; i < rest.length; i += 2) {
    cPairs.push(`${rest[i]} ${rest[i + 1]}`);
  }
  return `M${mx} ${my} C ${cPairs.join(" ")}`;
}
