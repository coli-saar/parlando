/** Result of rendering one browser audio quantum from buffered PCM samples. */
export interface PcmRenderResult {
  underrun: boolean;
  renderedSamples: number;
}

/** Mutable 24 kHz PCM queue with linear interpolation for device-rate playback. */
export class PcmPlaybackBuffer {
  private samples: number[] = [];
  private cursor = 0;

  /** Appends signed 16-bit PCM samples normalized to the Web Audio range. */
  append(pcm: Int16Array): void {
    for (const sample of pcm) this.samples.push(sample / 32768);
  }

  /** Returns the number of source-rate samples that remain available for playback. */
  availableSamples(): number {
    return Math.max(0, this.samples.length - Math.floor(this.cursor));
  }

  /** Renders one output quantum using linear interpolation between source samples. */
  render(output: Float32Array, sourceSamplesPerOutput: number): PcmRenderResult {
    let renderedSamples = 0;
    for (let index = 0; index < output.length; index += 1) {
      const leftIndex = Math.floor(this.cursor);
      if (leftIndex >= this.samples.length) {
        output.fill(0, index);
        break;
      }
      const rightIndex = Math.min(leftIndex + 1, this.samples.length - 1);
      const fraction = this.cursor - leftIndex;
      output[index] = this.samples[leftIndex] * (1 - fraction) + this.samples[rightIndex] * fraction;
      this.cursor += sourceSamplesPerOutput;
      renderedSamples += 1;
    }
    this.discardConsumed();
    return { underrun: renderedSamples < output.length, renderedSamples };
  }

  /** Drops oldest unplayed samples until no more than the requested amount remains. */
  trimTo(maxSamples: number): void {
    const excess = this.availableSamples() - Math.max(0, maxSamples);
    if (excess <= 0) return;
    this.samples.splice(0, excess);
    this.cursor = Math.max(0, this.cursor - excess);
  }

  /** Removes samples strictly before the current fractional interpolation cursor. */
  private discardConsumed(): void {
    const consumed = Math.floor(this.cursor);
    if (consumed <= 0) return;
    this.samples.splice(0, consumed);
    this.cursor -= consumed;
  }
}

if (typeof AudioWorkletProcessor !== "undefined") {
  /// Plays queued 24 kHz PCM16 through the local device sample rate.
  class ParlandoPlaybackProcessor extends AudioWorkletProcessor {
    private buffer = new PcmPlaybackBuffer();
    private started = false;
    private jitterSamples = 2_400;
    private resumeSamples = 960;
    private underruns = 0;

    /** Installs the PCM queue receiver for frames delivered by the relay. */
    constructor() {
      super();
      this.port.onmessage = (event: MessageEvent<ArrayBuffer | { jitterSamples: number }>) => {
        if (!(event.data instanceof ArrayBuffer)) {
          this.jitterSamples = Math.max(480, Math.round(event.data.jitterSamples));
          this.resumeSamples = Math.min(this.jitterSamples, 960);
          return;
        }
        this.buffer.append(new Int16Array(event.data));
      };
    }

    /** Fills one output quantum after the configured jitter buffer has accumulated. */
    process(_inputs: Float32Array[][], outputs: Float32Array[][]): boolean {
      const output = outputs[0]?.[0];
      if (!output) return true;
      const startThreshold = this.underruns === 0 ? this.jitterSamples : this.resumeSamples;
      if (!this.started && this.buffer.availableSamples() >= startThreshold) this.started = true;
      if (!this.started) { output.fill(0); return true; }
      const step = 24_000 / sampleRate;
      const result = this.buffer.render(output, step);
      if (result.underrun) {
        this.started = false;
        this.underruns += 1;
        this.port.postMessage({ type: "playbackUnderrun", count: this.underruns, bufferedSamples: this.buffer.availableSamples() });
      }
      if (this.buffer.availableSamples() > this.jitterSamples * 4) this.buffer.trimTo(this.jitterSamples);
      return true;
    }
  }

  registerProcessor("parlando-playback", ParlandoPlaybackProcessor);
}
