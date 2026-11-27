/**
 * Freight sound: synthesized with Web Audio, no sample files.
 *
 * Owns one AudioContext and a small graph: a diesel engine (detuned saw
 * oscillators through a low-pass filter with a chugging amplitude LFO), wheel
 * roar and rail clacks driven by distance travelled, one-shot couplings, a
 * two-tone horn, and a factory hum. The replay sets continuous levels each frame
 * through `setMix` and fires one-shots on events. The context starts suspended
 * until a user gesture (`resume`), exactly like the hotel's lift sound; every
 * method is a no-op when audio is unavailable, muted or the tab is hidden.
 */
export interface FreightSound {
  /** Unlocks audio after a user gesture. */
  resume(): void;
  /** Continuous levels: engine 0..1 (idle..full), wheel roar 0..1, factory hum 0..1, master 0..1 for the focus distance. */
  setMix(mix: { engine: number; wheels: number; factory: number }): void;
  /** A single rail joint clack; `gain` 0..1. */
  clack(gain: number): void;
  /** Coupling or uncoupling clank. */
  clank(): void;
  /** Loading thud. */
  thud(): void;
  /** Horn blast. */
  horn(): void;
  /** Silences everything and stops continuous sources. */
  setMuted(muted: boolean): void;
  dispose(): void;
}

/** Builds the sound system; returns an inert object if Web Audio cannot start. */
export function createFreightSound(): FreightSound {
  let ctx: AudioContext | undefined;
  try { ctx = new AudioContext(); } catch { ctx = undefined; }
  const inert: FreightSound = { resume() {}, setMix() {}, clack() {}, clank() {}, thud() {}, horn() {}, setMuted() {}, dispose() {} };
  if (!ctx) return inert;
  const ac = ctx;
  let muted = false;
  const master = ac.createGain();
  master.gain.value = 0.7;
  master.connect(ac.destination);

  /** One second of white noise reused by every noise source. */
  const noiseBuffer = ac.createBuffer(1, ac.sampleRate, ac.sampleRate);
  const data = noiseBuffer.getChannelData(0);
  for (let i = 0; i < data.length; i++) data[i] = Math.random() * 2 - 1;
  const noise = () => { const s = ac.createBufferSource(); s.buffer = noiseBuffer; s.loop = true; return s; };

  // Engine: two detuned saws, chug LFO.
  const engineGain = ac.createGain(); engineGain.gain.value = 0;
  const engineFilter = ac.createBiquadFilter(); engineFilter.type = "lowpass"; engineFilter.frequency.value = 220;
  const o1 = ac.createOscillator(); o1.type = "sawtooth"; o1.frequency.value = 36;
  const o2 = ac.createOscillator(); o2.type = "sawtooth"; o2.frequency.value = 54.5;
  const chug = ac.createGain(); chug.gain.value = 0.7;
  const lfo = ac.createOscillator(); lfo.frequency.value = 9;
  const lfoDepth = ac.createGain(); lfoDepth.gain.value = 0.3;
  lfo.connect(lfoDepth).connect(chug.gain);
  o1.connect(chug); o2.connect(chug); chug.connect(engineFilter).connect(engineGain).connect(master);
  // Wheels: band-passed noise.
  const wheelGain = ac.createGain(); wheelGain.gain.value = 0;
  const wheelFilter = ac.createBiquadFilter(); wheelFilter.type = "bandpass"; wheelFilter.frequency.value = 700; wheelFilter.Q.value = 0.6;
  const wn = noise(); wn.connect(wheelFilter).connect(wheelGain).connect(master);
  // Factory: low hum with a slow pulse.
  const factoryGain = ac.createGain(); factoryGain.gain.value = 0;
  const hum = ac.createOscillator(); hum.type = "square"; hum.frequency.value = 55;
  const humFilter = ac.createBiquadFilter(); humFilter.type = "lowpass"; humFilter.frequency.value = 160;
  const fl = ac.createOscillator(); fl.frequency.value = 2.2;
  const fd = ac.createGain(); fd.gain.value = 0.25; fl.connect(fd).connect(factoryGain.gain);
  hum.connect(humFilter).connect(factoryGain).connect(master);
  for (const n of [o1, o2, lfo, hum, fl]) n.start();
  wn.start();

  /** Plays a decaying burst: noise through a filter plus an optional sine. */
  const burst = (freq: number, q: number, dur: number, gain: number, sine?: number) => {
    if (muted || ac.state !== "running" || document.hidden) return;
    const t = ac.currentTime;
    const g = ac.createGain(); g.gain.setValueAtTime(gain, t); g.gain.exponentialRampToValueAtTime(0.0008, t + dur);
    const f = ac.createBiquadFilter(); f.type = "bandpass"; f.frequency.value = freq; f.Q.value = q;
    const n = noise(); n.connect(f).connect(g).connect(master); n.start(t); n.stop(t + dur + 0.05);
    if (sine) {
      const o = ac.createOscillator(); o.frequency.value = sine; o.type = "sine"; o.connect(g); o.start(t); o.stop(t + dur);
    }
  };
  const onVisibility = () => { if (document.hidden) setMix0(); };
  const setMix0 = () => { const t = ac.currentTime; for (const g of [engineGain, wheelGain, factoryGain]) g.gain.setTargetAtTime(0, t, 0.05); };
  document.addEventListener("visibilitychange", onVisibility);

  return {
    resume() { if (ac.state === "suspended") void ac.resume().catch(() => {}); },
    setMix({ engine, wheels, factory }) {
      if (muted || document.hidden) return setMix0();
      const t = ac.currentTime;
      engineGain.gain.setTargetAtTime(0.05 + engine * 0.22, t, 0.12);
      engineFilter.frequency.setTargetAtTime(160 + engine * 520, t, 0.15);
      o1.frequency.setTargetAtTime(34 + engine * 22, t, 0.2);
      o2.frequency.setTargetAtTime(51 + engine * 33, t, 0.2);
      lfo.frequency.setTargetAtTime(8 + engine * 6, t, 0.2);
      wheelGain.gain.setTargetAtTime(wheels * 0.18, t, 0.1);
      wheelFilter.frequency.setTargetAtTime(500 + wheels * 900, t, 0.1);
      factoryGain.gain.setTargetAtTime(factory * 0.1, t, 0.3);
    },
    clack(gain) { burst(1800, 1.5, 0.07, 0.25 * gain, 160); },
    clank() { burst(900, 3, 0.35, 0.5, 130); burst(2600, 4, 0.12, 0.25); },
    thud() { burst(200, 1, 0.25, 0.5, 70); },
    horn() {
      if (muted || ac.state !== "running" || document.hidden) return;
      const t = ac.currentTime;
      const g = ac.createGain();
      g.gain.setValueAtTime(0, t); g.gain.linearRampToValueAtTime(0.22, t + 0.08);
      g.gain.setValueAtTime(0.22, t + 1.0); g.gain.exponentialRampToValueAtTime(0.001, t + 1.5);
      const f = ac.createBiquadFilter(); f.type = "lowpass"; f.frequency.value = 1400;
      for (const hz of [311, 392, 466]) { const o = ac.createOscillator(); o.type = "sawtooth"; o.frequency.value = hz; o.connect(f); o.start(t); o.stop(t + 1.6); }
      f.connect(g).connect(master);
    },
    setMuted(m) { muted = m; master.gain.setTargetAtTime(m ? 0 : 0.7, ac.currentTime, 0.05); if (m) setMix0(); },
    dispose() {
      document.removeEventListener("visibilitychange", onVisibility);
      for (const n of [o1, o2, lfo, hum, fl, wn]) { try { n.stop(); } catch { /* already stopped */ } }
      void ac.close().catch(() => {});
    }
  };
}
