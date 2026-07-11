import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it, vi } from "vitest";
import { SpeechmaticsTranscriptionSink } from "./speechmaticsTranscriptionSink";
import type { AudioSessionContext, MicrophoneInput } from "./types";

class FakeWebSocket extends EventTarget {
  static instances: FakeWebSocket[] = [];
  static OPEN = 1;
  readyState = FakeWebSocket.OPEN;
  binaryType = "";
  sent: unknown[] = [];

  constructor(readonly url: string) {
    super();
    FakeWebSocket.instances.push(this);
  }

  send(data: unknown) {
    this.sent.push(data);
  }

  close() {
    this.readyState = 3;
  }
}

class FakeAudioContext {
  sampleRate = 16000;
  destination = {};

  async resume() {
    return undefined;
  }

  async close() {
    return undefined;
  }

  createMediaStreamSource() {
    return { connect: vi.fn(), disconnect: vi.fn() };
  }

  createScriptProcessor() {
    return { connect: vi.fn(), disconnect: vi.fn(), onaudioprocess: null };
  }
}

function microphoneInput(): MicrophoneInput {
  const track = { stop: vi.fn() } as unknown as MediaStreamTrack;
  return {
    deviceId: "default",
    deviceLabel: "Default microphone",
    stream: { getTracks: () => [track] } as unknown as MediaStream,
    track,
    createTrackClone: () => track,
    createMediaStream: () => ({ getTracks: () => [track] }) as unknown as MediaStream
  };
}

function context(): AudioSessionContext {
  return {
    roomId: "room",
    participantSessionId: "participant",
    role: "A",
    selectedAudioInputId: "default",
    selectedAudioInputLabel: "Default microphone",
    getAudioSession: vi.fn(async () => ({
      enabled: true,
      capture: { audio: true },
      sinks: [
        {
          id: "speechmatics-transcription",
          provider: "speechmatics",
          purposes: ["transcription"],
          transport: "websocket-stt",
          credentials: {
            enabled: true,
            realtime_url: "wss://eu.rt.speechmatics.com/v2",
            temporary_key: "temporary-key",
            language: "en",
            model: "enhanced",
            max_delay: 2,
            enable_partials: true,
            end_of_utterance_silence_trigger: 1.2
          }
        }
      ]
    })),
    getLiveKitToken: vi.fn(),
    postTranscriptSegment: vi.fn(async () => ({})),
    logVoice: vi.fn(),
    onVoiceStatus: vi.fn()
  };
}

describe("SpeechmaticsTranscriptionSink", () => {
  it("does not import LiveKit or call getUserMedia", () => {
    const source = readFileSync(fileURLToPath(new URL("./speechmaticsTranscriptionSink.ts", import.meta.url)), "utf8");

    expect(source).not.toContain("livekit");
    expect(source).not.toContain("getUserMedia");
  });

  it("starts a Speechmatics websocket session and posts final transcripts", async () => {
    FakeWebSocket.instances.length = 0;
    const originalWebSocket = globalThis.WebSocket;
    const originalAudioContext = globalThis.AudioContext;
    vi.stubGlobal("WebSocket", FakeWebSocket);
    vi.stubGlobal("AudioContext", FakeAudioContext);
    const audioContext = context();
    const sink = new SpeechmaticsTranscriptionSink();

    const connected = sink.connect(microphoneInput(), audioContext);
    await vi.waitFor(() => expect(FakeWebSocket.instances[0]).toBeDefined());
    const socket = FakeWebSocket.instances[0];
    socket.dispatchEvent(new Event("open"));
    await connected;
    socket.dispatchEvent(
      new MessageEvent("message", {
        data: JSON.stringify({
          message: "AddTranscript",
          metadata: { transcript: "Power is" },
          id: "result-1"
        })
      })
    );
    socket.dispatchEvent(
      new MessageEvent("message", {
        data: JSON.stringify({
          message: "AddTranscript",
          metadata: { transcript: "on." },
          id: "result-2"
        })
      })
    );
    expect(audioContext.postTranscriptSegment).not.toHaveBeenCalled();
    socket.dispatchEvent(
      new MessageEvent("message", {
        data: JSON.stringify({ message: "EndOfUtterance" })
      })
    );
    await Promise.resolve();

    expect(socket.url).toBe("wss://eu.rt.speechmatics.com/v2?jwt=temporary-key");
    expect(JSON.parse(socket.sent[0] as string)).toMatchObject({
      message: "StartRecognition",
      audio_format: { type: "raw", encoding: "pcm_s16le", sample_rate: 16000 },
      transcription_config: {
        language: "en",
        model: "enhanced",
        max_delay: 2,
        conversation_config: { end_of_utterance_silence_trigger: 1.2 }
      }
    });
    expect(audioContext.postTranscriptSegment).toHaveBeenCalledWith(
      expect.objectContaining({
        participant_session_id: "participant",
        player: "A",
        text: "Power is on.",
        metadata: expect.objectContaining({ provider: "speechmatics" })
      })
    );

    vi.stubGlobal("WebSocket", originalWebSocket);
    vi.stubGlobal("AudioContext", originalAudioContext);
  });
});
