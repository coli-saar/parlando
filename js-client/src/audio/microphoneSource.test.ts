import { describe, expect, it, vi } from "vitest";
import { MicrophoneSource } from "./microphoneSource";

interface FakeTrack extends MediaStreamTrack {
  stop: ReturnType<typeof vi.fn>;
  clone: ReturnType<typeof vi.fn>;
}

/** Creates a controllable browser audio track for ownership assertions. */
function track(label: string): FakeTrack {
  const cloned = { label: `${label} clone`, stop: vi.fn() } as unknown as MediaStreamTrack;
  return {
    label,
    stop: vi.fn(),
    clone: vi.fn(() => cloned)
  } as unknown as FakeTrack;
}

/** Creates the minimal stream contract consumed by MicrophoneSource. */
function stream(audioTrack?: FakeTrack): MediaStream {
  const tracks = audioTrack ? [audioTrack] : [];
  return {
    getAudioTracks: () => tracks,
    getTracks: () => tracks
  } as unknown as MediaStream;
}

/** Creates an externally resolvable promise for deterministic race ordering. */
function deferred<T>(): {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (reason: unknown) => void;
} {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

/** Creates a source without a real analyser so tests focus on media ownership. */
function source(getUserMedia: (constraints: MediaStreamConstraints) => Promise<MediaStream>): MicrophoneSource {
  return new MicrophoneSource({
    getUserMedia,
    isSecureContext: () => true,
    getAudioContext: () => undefined
  });
}

describe("MicrophoneSource", () => {
  it("fails clearly when input is requested before preparation", () => {
    expect(() => source(vi.fn()).input()).toThrow("has not been prepared");
  });

  it("requests exact devices, exposes clones, and reuses the same device", async () => {
    const selectedTrack = track("USB microphone");
    const selectedStream = stream(selectedTrack);
    const getUserMedia = vi.fn(async () => selectedStream);
    const microphone = source(getUserMedia);

    const first = await microphone.prepare("usb-1", "Fallback");
    const second = await microphone.prepare("usb-1", "Fallback");

    expect(getUserMedia).toHaveBeenCalledOnce();
    expect(getUserMedia).toHaveBeenCalledWith({ audio: { deviceId: { exact: "usb-1" } } });
    expect(second.stream).toBe(first.stream);
    expect(second.track).toBe(first.track);
    expect(first.deviceLabel).toBe("USB microphone");
    expect(first.createTrackClone()).toBe(selectedTrack.clone.mock.results[0]?.value);
  });

  it("uses the browser default constraint and fallback label", async () => {
    const unlabeledTrack = track("");
    const getUserMedia = vi.fn(async () => stream(unlabeledTrack));
    const microphone = source(getUserMedia);

    const input = await microphone.prepare("", "Studio input");

    expect(getUserMedia).toHaveBeenCalledWith({ audio: true });
    expect(input.deviceLabel).toBe("Studio input");
  });

  it("stops a stream that contains no audio track", async () => {
    const videoTrack = track("non-audio");
    const emptyStream = {
      getAudioTracks: () => [],
      getTracks: () => [videoTrack]
    } as unknown as MediaStream;
    const microphone = source(vi.fn(async () => emptyStream));

    await expect(microphone.prepare("bad")).rejects.toThrow("No microphone audio track");
    expect(videoTrack.stop).toHaveBeenCalledOnce();
    expect(microphone.snapshot().ready).toBe(false);
  });

  it("rejects insecure pages and browsers without media capture", async () => {
    const insecure = new MicrophoneSource({ isSecureContext: () => false });
    await expect(insecure.prepare("")).rejects.toThrow("requires HTTPS");
    expect(insecure.snapshot().message).toContain("requires HTTPS");

    const unavailable = new MicrophoneSource({
      isSecureContext: () => true,
      getUserMedia: undefined
    });
    await expect(unavailable.prepare("")).rejects.toThrow("does not expose microphone access");
  });

  it("allows only the newest concurrent preparation to own microphone state", async () => {
    const first = deferred<MediaStream>();
    const second = deferred<MediaStream>();
    const getUserMedia = vi.fn()
      .mockImplementationOnce(() => first.promise)
      .mockImplementationOnce(() => second.promise);
    const microphone = source(getUserMedia);
    const oldTrack = track("Old microphone");
    const newTrack = track("New microphone");

    const oldPreparation = microphone.prepare("old");
    const newPreparation = microphone.prepare("new");
    second.resolve(stream(newTrack));
    await expect(newPreparation).resolves.toMatchObject({ deviceId: "new", deviceLabel: "New microphone" });
    first.resolve(stream(oldTrack));
    await expect(oldPreparation).rejects.toThrow("superseded");

    expect(oldTrack.stop).toHaveBeenCalledOnce();
    expect(newTrack.stop).not.toHaveBeenCalled();
    expect(microphone.input().deviceId).toBe("new");
  });

  it("does not let a stale rejection clear a newer successful preparation", async () => {
    const first = deferred<MediaStream>();
    const second = deferred<MediaStream>();
    const microphone = source(vi.fn()
      .mockImplementationOnce(() => first.promise)
      .mockImplementationOnce(() => second.promise));
    const currentTrack = track("Current microphone");

    const oldPreparation = microphone.prepare("old");
    const currentPreparation = microphone.prepare("current");
    second.resolve(stream(currentTrack));
    await currentPreparation;
    first.reject(new DOMException("Permission denied", "NotAllowedError"));
    await expect(oldPreparation).rejects.toMatchObject({ name: "NotAllowedError" });

    expect(microphone.snapshot().ready).toBe(true);
    expect(microphone.input().deviceId).toBe("current");
  });

  it("invalidates and cleans up a pending preparation after stop", async () => {
    const pending = deferred<MediaStream>();
    const microphone = source(vi.fn(() => pending.promise));
    const pendingTrack = track("Late microphone");

    const preparation = microphone.prepare("late");
    microphone.stop();
    pending.resolve(stream(pendingTrack));

    await expect(preparation).rejects.toThrow("superseded");
    expect(pendingTrack.stop).toHaveBeenCalledOnce();
    expect(() => microphone.input()).toThrow("has not been prepared");
  });

  it("subscribes immediately, supports removal, and resets to the initial state", async () => {
    const microphone = source(vi.fn(async () => stream(track("Mic"))));
    const listener = vi.fn();
    const unsubscribe = microphone.subscribe(listener);
    await microphone.prepare("mic");
    unsubscribe();
    microphone.reset();

    expect(listener.mock.calls[0]?.[0].message).toBe("Voice not prepared");
    expect(listener).toHaveBeenCalledTimes(4);
    expect(microphone.snapshot()).toMatchObject({ requested: false, ready: false, micLevel: 0 });
  });

  it("continues track cleanup when probe and track teardown throw", async () => {
    const firstTrack = track("Broken teardown");
    firstTrack.stop.mockImplementation(() => { throw new Error("stop failed"); });
    const secondTrack = track("Still stopped");
    const disconnect = vi.fn(() => { throw new Error("disconnect failed"); });
    const close = vi.fn(async () => undefined);
    const FakeAudioContext = class {
      createAnalyser = () => ({ fftSize: 1, smoothingTimeConstant: 0, getByteTimeDomainData: vi.fn() });
      createMediaStreamSource = () => ({ connect: vi.fn(), disconnect });
      resume = async () => undefined;
      close = close;
    } as unknown as typeof AudioContext;
    const microphone = new MicrophoneSource({
      getUserMedia: vi.fn(async () => ({
        getAudioTracks: () => [firstTrack],
        getTracks: () => [firstTrack, secondTrack]
      } as unknown as MediaStream)),
      isSecureContext: () => true,
      getAudioContext: () => FakeAudioContext,
      requestAnimationFrame: () => 1,
      cancelAnimationFrame: vi.fn()
    });

    await microphone.prepare("mic");
    expect(() => microphone.stop()).not.toThrow();
    expect(disconnect).toHaveBeenCalledOnce();
    expect(firstTrack.stop).toHaveBeenCalledOnce();
    expect(secondTrack.stop).toHaveBeenCalledOnce();
    expect(close).toHaveBeenCalledOnce();
  });

  it("keeps microphone readiness when analyser setup is unavailable", async () => {
    const selectedTrack = track("Usable microphone");
    const FakeAudioContext = class {
      createAnalyser = () => { throw new Error("analyser unavailable"); };
    } as unknown as typeof AudioContext;
    const microphone = new MicrophoneSource({
      getUserMedia: vi.fn(async () => stream(selectedTrack)),
      isSecureContext: () => true,
      getAudioContext: () => FakeAudioContext
    });

    await expect(microphone.prepare("mic")).resolves.toMatchObject({ track: selectedTrack });
    expect(microphone.snapshot()).toMatchObject({ ready: true, micProbeActive: false });
  });
});
