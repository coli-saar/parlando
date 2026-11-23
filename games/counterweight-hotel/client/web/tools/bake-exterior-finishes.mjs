// Run with Node.js and cwebp available. Writes the maps used by exterior-finishes.ts.
import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

/** Maps an integer lattice to independently distributed, repeatable samples in [0, 1). */
function lattice(x, y) {
  let h = Math.imul(x + 41, 374761393) ^ Math.imul(y + 13, 668265263);
  h = Math.imul(h ^ (h >>> 13), 1274126177);
  return ((h ^ (h >>> 16)) >>> 0) / 4294967296;
}

/** Interpolates a periodic lattice. Anisotropic frequencies shape grain, drifts and strata. */
function noise(u, v, columns, rows) {
  const x = u * columns, y = v * rows;
  const i = Math.floor(x), j = Math.floor(y);
  const xp = (i + columns) % columns, yp = (j + rows) % rows;
  const xn = (xp + 1) % columns, yn = (yp + 1) % rows;
  const f = x - i, g = y - j;
  const a = f * f * (3 - 2 * f), b = g * g * (3 - 2 * g);
  const low = lattice(xp, yp) * (1 - a) + lattice(xn, yp) * a;
  const high = lattice(xp, yn) * (1 - a) + lattice(xn, yn) * a;
  return low * (1 - b) + high * b - 0.5;
}

/** Gives masonry mortar joints an uneven edge and varies the colour of individual stones. */
function ashlar(u, v, roughness, fine) {
  const course = Math.floor(v * 8);
  const across = u * 4 + (course % 2) * 0.5 + lattice(11, course) * 0.22;
  const block = Math.floor(across);
  const dx = across - block, dy = v * 8 - course;
  const edge = Math.min(Math.min(dx, 1 - dx) * 2.8, dy, 1 - dy);
  const crown = Math.max(0, Math.min(1, (edge + fine * 0.023 - 0.018) * 47));
  const stone = lattice(block % 4, course) - 0.5;
  const tint = stone * 29 + roughness * 22 + fine * 17;
  const shade = 0.59 + 0.41 * crown;
  return [(174 + tint) * shade, (163 + tint) * shade, (146 + tint * 0.95) * shade,
    crown * (0.58 + stone * 0.11 + roughness * 0.22) + fine * 0.14];
}

/** Samples pigment and relief together so flakes, pits and aggregate agree under directional light. */
function sample(surface, u, v) {
  const cloud = noise(u, v, 4, 4);
  const roughness = noise(u, v, 19, 19);
  const fine = noise(u, v, 83, 83);
  if (surface === "ashlar") return ashlar(u, v, roughness + cloud * 0.54, fine);
  if (surface === "dressing" || surface === "flagstone") {
    const warp = noise(u, v, 7, 9);
    const grain = noise(u, v + warp * 0.02, 9, 29);
    const aggregate = Math.max(0, fine - 0.23) * 42;
    const tint = cloud * 31 + roughness * 18 + grain * 16 + fine * 10 + aggregate;
    const base = surface === "flagstone" ? [152, 148, 137] : [185, 174, 155];
    return [base[0] + tint, base[1] + tint, base[2] + tint * 0.92,
      roughness * 0.31 + grain * 0.10 + fine * 0.17];
  }
  if (surface === "joinery" || surface === "deck") {
    // Warp fibres around two elliptical knots in each tile without seams at the tile edge.
    const wrapped = Math.abs(u - 0.54);
    const knot = Math.exp(-((wrapped * 19) ** 2 + ((v - 0.41) * 12) ** 2))
      + Math.exp(-(((u - 0.29) * 26) ** 2 + ((v - 0.77) * 19) ** 2));
    const warp = noise(u, v, 3, 8) * 0.053 + knot * 0.072;
    const fibre = noise(u, v + warp, 4, 91);
    const growth = Math.sin(Math.PI * 2 * (v + warp) * 24);
    const tint = fibre * 25 + growth * 7 + cloud * 20 + fine * 9 - knot * 42;
    const base = surface === "deck" ? [126, 99, 73] : [105, 60, 48];
    return [base[0] + tint, base[1] + tint * 0.88, base[2] + tint * 0.76,
      fibre * 0.30 + growth * 0.045 - knot * 0.21 + fine * 0.06];
  }
  if (surface === "steel") {
    const scrape = noise(u, v, 55, 9);
    const tint = roughness * 10 + fine * 8 + cloud * 7;
    const chip = Math.max(0, scrape + fine * 0.38 - 0.32) * 5;
    return [63 + tint + chip * 68, 82 + tint + chip * 26, 76 + tint - chip * 2,
      fine * 0.16 + roughness * 0.10 - chip * 0.13];
  }
  if (surface === "scree") {
    const layer = noise(u, v + noise(u, v, 4, 7) * 0.048, 12, 40);
    const fracture = Math.max(0, Math.abs(layer) - 0.18);
    const tint = cloud * 45 + roughness * 33 + fine * 27 + layer * 36 - fracture * 40;
    return [99 + tint, 103 + tint, 101 + tint * 0.88,
      layer * 0.46 + roughness * 0.28 + fine * 0.21];
  }
  const layer = noise(u, v + noise(u, v, 3, 5) * 0.044, 6, 29);
  const ice = surface === "ice";
  const tint = cloud * 18 + roughness * 11 + fine * 16 + layer * (ice ? 42 : 12);
  const base = ice ? [155, 190, 205] : [223, 233, 238];
  return [base[0] + tint, base[1] + tint, base[2] + tint * 0.8,
    layer * (ice ? 0.34 : 0.11) + roughness * 0.18 + fine * 0.14];
}

/** Writes RGB tiles and encodes them as mipmap-friendly WebP source assets. */
function bake(surface, size, scratch) {
  const pigment = new Uint8ClampedArray(size * size * 3);
  const relief = new Uint8ClampedArray(size * size * 3);
  const heights = new Float32Array(size * size);
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      const i = y * size + x;
      const [r, g, b, height] = sample(surface, x / size, y / size);
      pigment.set([r, g, b], i * 3);
      heights[i] = height;
    }
  }
  const slope = { snow: 0.46, ice: 0.69, scree: 0.82 }[surface] ?? 1;
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      const dx = slope * (heights[y * size + ((x + size - 1) % size)] - heights[y * size + ((x + 1) % size)]);
      const dy = slope * (heights[((y + 1) % size) * size + x] - heights[((y + size - 1) % size) * size + x]);
      const length = Math.hypot(dx, dy, 1);
      relief.set([128 + 127 * dx / length, 128 + 127 * dy / length, 128 + 127 / length],
        (y * size + x) * 3);
    }
  }
  for (const [kind, tile, encoder] of [
    ["colour", pigment, ["-q", "87"]],
    ["normal", relief, ["-near_lossless", "80"]],
  ]) {
    const input = join(scratch, kind + ".ppm");
    const output = fileURLToPath(new URL("../src/art/exterior/" + surface + "-" + kind + ".webp", import.meta.url));
    writeFileSync(input, Buffer.concat([Buffer.from(`P6\n${size} ${size}\n255\n`), Buffer.from(tile)]));
    execFileSync("cwebp", ["-quiet", ...encoder, input, "-o", output]);
  }
}

const scratch = mkdtempSync(join(tmpdir(), "hotel-exterior-"));
try {
  for (const [surface, size] of [
    ["ashlar", 512], ["dressing", 256], ["flagstone", 512], ["joinery", 256], ["deck", 256],
    ["steel", 256], ["snow", 256], ["ice", 256], ["scree", 512],
  ]) bake(surface, size, scratch);
} finally {
  rmSync(scratch, { recursive: true, force: true });
}
