/// Resamples microphone input to 24 kHz PCM16 frames.
class ParlandoCaptureProcessor extends AudioWorkletProcessor {
  private output: number[] = [];
  private phase = 0;
  /** Converts one browser render quantum and emits complete 20 ms PCM frames. */
  process(inputs: Float32Array[][]): boolean {
    const input = inputs[0]?.[0];
    if (!input) return true;
    const ratio = sampleRate / 24_000;
    while (this.phase < input.length) {
      const left = Math.floor(this.phase);
      const fraction = this.phase - left;
      const value = input[left] * (1 - fraction) + input[Math.min(left + 1, input.length - 1)] * fraction;
      this.output.push(Math.max(-1, Math.min(1, value)));
      this.phase += ratio;
      if (this.output.length === 480) {
        const pcm = new Int16Array(480);
        for (let index = 0; index < pcm.length; index += 1) pcm[index] = this.output[index] < 0 ? Math.round(this.output[index] * 32768) : Math.round(this.output[index] * 32767);
        this.port.postMessage(pcm.buffer, [pcm.buffer]);
        this.output = [];
      }
    }
    this.phase -= input.length;
    return true;
  }
}
registerProcessor("parlando-capture", ParlandoCaptureProcessor);
