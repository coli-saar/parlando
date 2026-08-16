/** Stateful browser-rate to 24 kHz PCM16 frame converter. */
export class PcmCaptureResampler {
  private samples: number[] = [];
  private cursor = 0;
  private output: number[] = [];

  /** Appends one browser render quantum and returns every completed 20 ms frame. */
  append(input: Float32Array, inputSampleRate: number): ArrayBuffer[] {
    if (!Number.isFinite(inputSampleRate) || inputSampleRate <= 0) {
      throw new Error("inputSampleRate must be finite and positive.");
    }
    for (const sample of input) this.samples.push(Math.max(-1, Math.min(1, sample)));
    const ratio = inputSampleRate / 24_000;
    const frames: ArrayBuffer[] = [];
    while (this.cursor < this.samples.length) {
      const left = Math.floor(this.cursor);
      const fraction = this.cursor - left;
      if (fraction > 0 && left + 1 >= this.samples.length) break;
      const right = Math.min(left + 1, this.samples.length - 1);
      const value = this.samples[left] * (1 - fraction) + this.samples[right] * fraction;
      this.output.push(Math.max(-1, Math.min(1, value)));
      this.cursor += ratio;
      if (this.output.length === 480) {
        const pcm = new Int16Array(480);
        for (let index = 0; index < pcm.length; index += 1) {
          pcm[index] = this.output[index] < 0
            ? Math.round(this.output[index] * 32768)
            : Math.round(this.output[index] * 32767);
        }
        frames.push(pcm.buffer);
        this.output = [];
      }
    }
    const consumed = Math.min(Math.floor(this.cursor), this.samples.length);
    if (consumed > 0) {
      this.samples.splice(0, consumed);
      this.cursor -= consumed;
    }
    return frames;
  }
}

if (typeof AudioWorkletProcessor !== "undefined") {
  /** Resamples microphone input to 24 kHz PCM16 frames. */
  class ParlandoCaptureProcessor extends AudioWorkletProcessor {
    private resampler = new PcmCaptureResampler();

    /** Converts one browser render quantum and emits complete 20 ms PCM frames. */
    process(inputs: Float32Array[][]): boolean {
      const input = inputs[0]?.[0];
      if (!input) return true;
      for (const frame of this.resampler.append(input, sampleRate)) {
        this.port.postMessage(frame, [frame]);
      }
      return true;
    }
  }

  registerProcessor("parlando-capture", ParlandoCaptureProcessor);
}
