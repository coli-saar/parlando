/** Plays a local mechanical loop for each direction. Both guests hear their own car,
 * regardless of camera orientation. Voice and other session audio have independent routing. */
export function createLiftSound() {
  let context: AudioContext | undefined;
  try { context = new AudioContext(); } catch { /* The lift also runs on devices without an audio output. */ }
  const clips = new Map<number, AudioBuffer>();
  let direction = 0;
  let playing: { source: AudioBufferSourceNode; gain: GainNode } | undefined;
  let disposed = false;

  /** Releases a loop with a short tail so braking or reversing cannot cut a waveform mid-cycle. */
  function stop() {
    if (!context || !playing) return;
    const { source, gain } = playing;
    playing = undefined;
    gain.gain.cancelScheduledValues(context.currentTime);
    gain.gain.setTargetAtTime(0, context.currentTime, 0.015);
    source.stop(context.currentTime + 0.08);
  }

  /** Starts only the currently requested direction, including after audio loads or unlocks. */
  function sync() {
    const clip = clips.get(direction);
    if (!context || context.state !== "running" || document.hidden || disposed || !clip || playing) return;
    const source = context.createBufferSource();
    const gain = context.createGain();
    source.buffer = clip;
    source.loop = true;
    gain.gain.value = 0;
    gain.gain.setTargetAtTime(0.44, context.currentTime, 0.016);
    source.connect(gain).connect(context.destination);
    source.onended = () => { source.disconnect(); gain.disconnect(); };
    source.start();
    playing = { source, gain };
  }

  /** A gesture on the canvas, controls or chat can unlock audio on mobile browsers. */
  function unlock() {
    if (context?.state === "suspended") void context.resume().then(sync).catch(() => {});
  }

  /** Stop immediately on a hidden tab, even if its renderer is already suspended. */
  function onVisibility() {
    if (document.hidden) {
      direction = 0;
      stop();
    }
  }

  /** Decodes each small mechanical recording once for the lifetime of this scene. */
  async function load(key: number, url: URL) {
    if (!context) return;
    const response = await fetch(url);
    if (!response.ok || disposed) return;
    const clip = await context.decodeAudioData(await response.arrayBuffer());
    if (disposed) return;
    clips.set(key, clip);
    sync();
  }

  void load(1, new URL("../art/lift-ascent.wav", import.meta.url)).catch(() => {});
  void load(-1, new URL("../art/lift-descent.wav", import.meta.url)).catch(() => {});
  window.addEventListener("pointerup", unlock, true);
  window.addEventListener("keydown", unlock, true);
  document.addEventListener("visibilitychange", onVisibility);
  context?.addEventListener("statechange", sync);
  if (navigator.userActivation?.hasBeenActive) unlock();

  return {
    /** Sounds the double impact of the bridge armature against its stop and latch. */
    relayClack() {
      if (!context || context.state !== "running" || document.hidden || disposed) return;
      const sample = context.createBuffer(1, Math.ceil(context.sampleRate * 0.14), context.sampleRate);
      const data = sample.getChannelData(0);
      for (let i = 0; i < data.length; i++) {
        const t = i / sample.sampleRate;
        const hit = t < 0.039 ? t : t - 0.039;
        const strength = t < 0.039 ? 0.37 : 0.21;
        data[i] = strength * Math.exp(-hit * 76) * (Math.sin(2 * Math.PI * 660 * hit) +
          0.42 * Math.sin(2 * Math.PI * 1452 * hit) + 0.30 * (Math.random() * 2 - 1));
      }
      const source = context.createBufferSource();
      source.buffer = sample;
      source.connect(context.destination);
      source.onended = () => source.disconnect();
      source.start();
    },
    /** Signed distance to the rendered lift's destination, zero when docked. */
    update(travel: number) {
      const next = document.hidden ? 0 : Math.sign(travel);
      if (direction === next) return;
      stop();
      direction = next;
      sync();
    },
    /** Releases any active sound and removes this scene’s input listeners. */
    dispose() {
      disposed = true;
      stop();
      window.removeEventListener("pointerup", unlock, true);
      window.removeEventListener("keydown", unlock, true);
      document.removeEventListener("visibilitychange", onVisibility);
      context?.removeEventListener("statechange", sync);
      if (context && context.state !== "closed") void context.close().catch(() => {});
    },
  };
}
