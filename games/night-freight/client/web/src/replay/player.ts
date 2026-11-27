/**
 * Replay player: plays a `Film` on the 3D world and publishes what the HTML
 * overlays show (clock, order sheet, schematic map positions, caption).
 *
 * Owns the film clock, shot selection, camera placement, sound cues for
 * events that pass during a shot, and the end of the film. It is created when a
 * run arrives (or after a reload while the phase is `replaying`) and always
 * starts from the beginning, so it needs only `view.last_run`.
 * Contract: `onEnd` is called once when the film reaches its end; for a film
 * that needs acknowledgement the player then holds the last frame and reports
 * `ended: true` in its view so the UI can offer "Back to planning".
 */
import type { LastRun, OrderState } from "../game";
import type { Geography } from "../scene/geography";
import type { NightFreightWorld } from "../scene/world";
import { direct, type Film, type Shot, type Tone } from "./director";
import { workLabel } from "./narrate";
import { Timeline } from "./timeline";

/** One order row on the film's order sheet. */
export interface SheetOrder extends Pick<OrderState, "id" | "commodity" | "count" | "to" | "deadline"> { status: "open" | "met" | "missed" }

/** Marker on the schematic map. */
export interface MapMarker { id: string; x: number; z: number }

/** Snapshot of everything the overlays need. */
export interface ReplayView {
  clockHour: number;
  caption: string | null;
  note: string | null;
  /** Tone of the caption (failures are drawn in warning colours). */
  tone: Tone;
  /** True while sim time runs fast between happenings: the HUD shows "fast-forward". */
  fast: boolean;
  /** Progress cards of the ops running now (loading, processing, delivering). */
  work: { id: string; label: string; progress: number }[];
  orders: SheetOrder[];
  markers: MapMarker[];
  ended: boolean;
  needsAcknowledge: boolean;
  shotIndex: number;
  shotCount: number;
  filmSeconds: number;
  totalSeconds: number;
  /** Highlights the order sheet (final failure shot). */
  sheetEmphasis: boolean;
}

/** Plays one run on a world. */
export class ReplayPlayer {
  readonly film: Film;
  private readonly actual: Timeline;
  private filmT = 0;
  private lastShot = -1;
  private lastSim = Number.NaN;
  private ended = false;
  private publishAt = 0;
  private stopped = false;
  /** Development-only: film second the clock is frozen at (see the `__replay` hook), or null. */
  private frozenAt: number | null = null;
  /** Development-only film speed multiplier (`?filmspeed=8`), used to inspect late shots quickly. */
  private readonly speed = import.meta.env.DEV ? Number(new URLSearchParams(window.location.search).get("filmspeed") ?? 1) || 1 : 1;

  constructor(
    private readonly world: NightFreightWorld,
    private readonly run: LastRun,
    private readonly geo: Geography,
    engineOrder: string[],
    names: (id: string) => string,
    private readonly onView: (v: ReplayView) => void,
    private readonly onEnd: () => void
  ) {
    this.actual = new Timeline(run.actual, geo, engineOrder);
    this.film = direct(run, this.actual, geo, names);
    world.clearStock();
    world.useFilm();
    world.setTicker((dt) => this.tick(dt));
    if (import.meta.env.DEV) (window as unknown as { __replay?: unknown }).__replay = {
      film: this.film,
      /** Development-only: jumps the film clock to `t` seconds and freezes it there (`seek(null)` resumes). */
      seek: (t: number | null) => { this.frozenAt = t; if (t !== null) this.filmT = t; }
    };
  }

  /** Ends playback and detaches from the world. */
  stop(): void {
    this.stopped = true;
    this.world.setTicker(null);
  }

  /** Index of the shot containing film time `t` and the film second within it. */
  private locate(t: number): { index: number; f: number } {
    let acc = 0;
    for (let i = 0; i < this.film.shots.length; i++) {
      const s = this.film.shots[i];
      if (t < acc + s.duration) return { index: i, f: t - acc };
      acc += s.duration;
    }
    const last = this.film.shots[this.film.shots.length - 1];
    return { index: this.film.shots.length - 1, f: last.duration };
  }

  /** Advances the film by `dt` seconds and draws. */
  private tick(dt: number): void {
    if (this.stopped || !this.film.shots.length) return;
    if (this.frozenAt !== null) this.filmT = this.frozenAt;
    else if (!this.ended) this.filmT += dt * this.speed;
    const { index, f } = this.locate(this.filmT);
    const shot: Shot = this.film.shots[index];
    const cut = index !== this.lastShot;
    if (cut) this.world.cut();
    this.lastShot = index;
    const { t, fast } = shot.at(f);
    this.world.setCamera(shot.camera(f));
    const frame = this.actual.evaluate(t);
    this.world.showFrame(frame, cut ? 0 : dt);
    this.cues(t, cut);
    this.mix(frame.t);
    if (!this.ended && this.filmT >= this.film.duration) {
      this.ended = true;
      this.onEnd();
    }
    const now = performance.now();
    if (now >= this.publishAt || cut || this.ended) {
      this.publishAt = now + 90;
      this.publish(index, shot, frame, t, fast);
    }
  }

  /** Fires one-shot sounds for events that passed since the previous frame when near the camera. */
  private cues(t: number, cut: boolean): void {
    const previous = this.lastSim;
    this.lastSim = t;
    if (cut || Number.isNaN(previous) || t <= previous) return;
    const cam = this.world.cameraPosition;
    for (const e of this.actual.events) {
      if (e.t <= previous || e.t > t) continue;
      const k = e.kind;
      let where: { x: number; z: number } | null = null;
      const st = "station" in k ? this.geo.stations.get(k.station) : undefined;
      if (st) where = st;
      else if (k.type === "depart") where = this.geo.stations.get(k.from) ?? null;
      if (!where || Math.hypot(where.x - cam.x, where.z - cam.z) > 280) continue;
      if (k.type === "depart") this.world.sound.horn();
      else if (k.type === "op_done" && (k.op.type === "couple" || k.op.type === "uncouple")) this.world.sound.clank();
      else if (k.type === "op_done" && (k.op.type === "load" || k.op.type === "process" || k.op.type === "deliver")) this.world.sound.thud();
    }
  }

  /** Sets continuous sound levels from the speed near the camera and proximity to the factory. */
  private mix(_t: number): void {
    const speed = this.world.speedNearCamera;
    const cam = this.world.cameraPosition;
    const factory = [...this.geo.stations.values()].find((s) => s.factory);
    const near = factory ? Math.max(0, 1 - Math.hypot(factory.x - cam.x, factory.z - cam.z) / 220) : 0;
    this.world.sound.setMix({ engine: 0.15 + Math.min(0.85, speed / 28), wheels: Math.min(1, speed / 24), factory: near * 0.7 });
  }

  /** Builds the overlay view. */
  private publish(index: number, shot: Shot, frame: ReturnType<Timeline["evaluate"]>, t: number, fast: boolean): void {
    const cap = [...shot.captions].reverse().find((c) => c.t0 <= t + 1e-9 && t < c.t1) ?? null;
    const start = this.run.actual.initial.hour;
    const orders = this.run.actual.initial.orders.map((o): SheetOrder => ({
      id: o.id, commodity: o.commodity, count: o.count, to: o.to, deadline: o.deadline,
      status: frame.met.includes(o.id) || o.delivered >= o.count ? "met" : frame.missed.includes(o.id) ? "missed" : "open"
    }));
    const markers: MapMarker[] = frame.engines.map((e) => ({ id: e.id, x: e.pos.x, z: e.pos.z }));
    this.onView({
      clockHour: Math.max(start, this.actual.trueHour(frame.t)), caption: cap?.text ?? null, note: cap?.note ?? null,
      tone: cap?.tone ?? "info", fast,
      work: frame.work.filter((w) => w.op.type === "load" || w.op.type === "process" || (w.op.type === "deliver" && w.hours > 0))
        .map((w) => ({ id: `${w.engine}${w.op.type}${w.station}`, label: `${w.engine} · ${workLabel(w.op, w.hours)}`, progress: w.progress })),
      orders, markers,
      ended: this.ended, needsAcknowledge: this.film.needsAcknowledge, shotIndex: index, shotCount: this.film.shots.length,
      filmSeconds: Math.min(this.filmT, this.film.duration), totalSeconds: this.film.duration, sheetEmphasis: shot.kind === "orders"
    });
  }
}
