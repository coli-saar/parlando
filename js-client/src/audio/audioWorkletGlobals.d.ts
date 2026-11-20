/**
 * These ambient declarations describe the worklet host used when compiling the capture and
 * playback processors. The browser supplies the processor base class, device sample rate, and
 * registration function when ParlandoAudioSink loads the worklet modules.
 */

/// Minimal worklet-global declarations omitted from TypeScript's DOM library.
declare abstract class AudioWorkletProcessor {
  readonly port: MessagePort;
  constructor(options?: AudioWorkletNodeOptions);
  abstract process(inputs: Float32Array[][], outputs: Float32Array[][], parameters: Record<string, Float32Array>): boolean;
}

declare const sampleRate: number;
declare function registerProcessor(name: string, processorCtor: new () => AudioWorkletProcessor): void;
