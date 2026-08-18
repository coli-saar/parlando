import type { LimbId, RootId } from "./types";

export interface AnchorPoint {
  x: number;
  y: number;
  rotation: number;
}

export interface LimbGeometry {
  /** One or more bark strokes; a forked limb (like Fork) has more than one. */
  barkPaths: { d: string; width: number }[];
  leaf: AnchorPoint;
  /** Where the gate ring is anchored — the trunk/limb junction, the click target. */
  gate: AnchorPoint;
  /** Where the bud/blossom is anchored — the far tip of the limb, away from the trunk. This is
   * deliberately a different point from `gate`: a flower blooms at the end of a branch, not at
   * its base. */
  bud: AnchorPoint;
  /** The always-visible trunk conduit for this limb, running to the bottom of the frame. */
  channelPath: string;
}

export interface RootGeometry {
  bodyPaths: { d: string; width: number }[];
  gate: AnchorPoint;
  /** Local ice-shard placement, used only while the root is iced. */
  iceShards: { rotation: number; length: number; count: number };
  /** The always-visible trunk conduit for this root, running to the top of the frame. */
  channelPath: string;
}

export const LIMB_GEOMETRY: Record<LimbId, LimbGeometry> = {
  spire: {
    // Hugs the trunk (stays left of x~90) until well above Nub's stub (y<65), then swings out
    // to its tip — Nub occupies roughly x100-160, y90-100, and a straighter rise here used to
    // pass directly through that band.
    barkPaths: [{ d: "M96 132 C 90 108 86 85 90 65 C 96 50 112 42 138 38", width: 11 }],
    leaf: { x: 89, y: 88, rotation: -35 },
    gate: { x: 96, y: 132, rotation: -50 },
    bud: { x: 138, y: 38, rotation: 4 },
    channelPath: "M96 132 C 68 170 38 235 33 300"
  },
  hook: {
    barkPaths: [
      {
        d: "M96 176 C 148 170 200 158 238 136 C 264 120 290 126 290 148 C 290 166 272 174 262 162",
        width: 13
      }
    ],
    leaf: { x: 176, y: 162, rotation: -12 },
    gate: { x: 96, y: 176, rotation: -7 },
    bud: { x: 262, y: 162, rotation: -8 },
    channelPath: "M96 176 C 82 200 56 250 52 300"
  },
  fork: {
    barkPaths: [
      { d: "M94 220 C 146 224 196 222 234 212", width: 11 },
      { d: "M234 212 C 266 198 298 188 334 188", width: 9 },
      { d: "M234 212 C 264 224 296 234 330 234", width: 9 }
    ],
    leaf: { x: 300, y: 196, rotation: 10 },
    gate: { x: 94, y: 220, rotation: 4 },
    bud: { x: 334, y: 188, rotation: 18 },
    channelPath: "M94 220 C 88 235 74 270 71 300"
  },
  cradle: {
    barkPaths: [
      { d: "M90 264 C 140 282 198 286 246 272 C 286 260 316 256 344 266", width: 12 }
    ],
    leaf: { x: 150, y: 260, rotation: -4 },
    gate: { x: 90, y: 264, rotation: 20 },
    bud: { x: 344, y: 266, rotation: 0 },
    channelPath: "M90 264 C 91 275 87 290 88 300"
  },
  nub: {
    barkPaths: [{ d: "M100 100 C 124 92 144 90 160 92", width: 9 }],
    leaf: { x: 118, y: 96, rotation: -8 },
    gate: { x: 100, y: 100, rotation: -18 },
    bud: { x: 160, y: 92, rotation: 0 },
    channelPath: "M100 100 C 65 150 25 220 15 300"
  }
};

// All five collars sit ON the trunk-flare's bottom boundary (computed from that path's own
// bezier curve — see the note above ROOT_GEOMETRY's declaration in the design notes), and are
// spread with generous clearance between neighbours. Earlier coordinates placed Deep's collar
// past the boundary entirely (a visible gap to the trunk) and let Knot's loop and Tip overlap;
// both are fixed by these positions, not by any change to each root's own silhouette.
export const ROOT_GEOMETRY: Record<RootId, RootGeometry> = {
  hand: {
    bodyPaths: [
      { d: "M129 53 C 113 87 99 115 87 143", width: 31 },
      { d: "M87 143 C 79 165 73 183 69 199", width: 23 },
      { d: "M69 199 C 55 221 43 241 33 263", width: 14 },
      { d: "M69 199 C 67 225 65 249 63 275", width: 14 },
      { d: "M69 199 C 85 221 99 239 111 257", width: 12 }
    ],
    gate: { x: 135, y: 57, rotation: -40 },
    iceShards: { rotation: 115, length: 90, count: 5 },
    channelPath: "M135 -8 C 135 18 135 40 135 57"
  },
  knot: {
    bodyPaths: [
      {
        d: "M180 67 C 176 95 170 113 160 127 C 142 151 152 183 180 179 C 202 176 206 153 188 145 C 192 137 196 103 200 69",
        width: 25
      },
      { d: "M180 179 C 184 209 190 237 194 263 C 196 279 198 289 200 298", width: 17 }
    ],
    gate: { x: 180, y: 67, rotation: -10 },
    iceShards: { rotation: 98, length: 90, count: 6 },
    channelPath: "M180 -8 C 180 18 180 40 180 67"
  },
  tip: {
    bodyPaths: [{ d: "M225 67 C 223 87 222 105 220 121", width: 15 }],
    gate: { x: 225, y: 67, rotation: 10 },
    iceShards: { rotation: 100, length: 48, count: 2 },
    channelPath: "M225 -8 C 225 16 225 40 225 67"
  },
  swollen: {
    bodyPaths: [
      { d: "M256 56 C 270 80 280 98 288 116", width: 27 },
      { d: "M300 178 C 306 206 310 232 312 260", width: 17 },
      { d: "M312 260 C 314 276 314 288 314 298", width: 9 }
    ],
    gate: { x: 260, y: 60, rotation: 22 },
    iceShards: { rotation: 60, length: 120, count: 7 },
    channelPath: "M260 -8 C 259 18 259 40 260 60"
  },
  deep: {
    bodyPaths: [
      { d: "M291 39 C 325 69 353 111 369 159", width: 24 },
      { d: "M369 159 C 379 193 383 231 381 285", width: 16 }
    ],
    gate: { x: 295, y: 43, rotation: 52 },
    iceShards: { rotation: 41, length: 120, count: 6 },
    channelPath: "M295 -8 C 294 12 294 28 295 43"
  }
};
