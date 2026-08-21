import { afterEach, describe, expect, it, vi } from "vitest";
import fixtures from "../../proto/participant_protocol_v1.fixtures.json";
import { apiBase, checkedJson, ParticipantClient, playerMessage, socketUrl, type ServerMessage } from "./protocol";

/** Creates an externally resolvable response promise for request-order races. */
function deferredResponse(): { promise: Promise<Response>; resolve: (response: Response) => void } {
  let resolve!: (response: Response) => void;
  const promise = new Promise<Response>((resolvePromise) => {
    resolve = resolvePromise;
  });
  return { promise, resolve };
}

/** Creates one valid direct participant response. */
function participantResponse(id: string, credential: string): Response {
  return new Response(JSON.stringify({
    participant_credential: credential,
    participant_id: `research-${id}`
  }), { status: 200 });
}

describe("ParticipantClient room helpers", () => {
  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("resolves experiment-relative WebSocket paths against the current origin", () => {
    vi.stubGlobal("window", { location: { origin: "https://study.example" } });

    expect(socketUrl("/e/pilot/ws/game/ROOM1", "ticket")).toBe(
      "wss://study.example/e/pilot/ws/game/ROOM1?token=ticket"
    );
  });

  it("derives API scope from only the leading experiment route", () => {
    vi.stubGlobal("window", {
      location: {
        origin: "https://study.example:8443",
        pathname: "/e/pilot/api/nested"
      }
    });
    expect(apiBase()).toBe("https://study.example:8443/e/pilot");

    window.location.pathname = "/unscoped";
    expect(apiBase()).toBe("https://study.example:8443");
  });

  it("replaces existing tokens and preserves unrelated WebSocket query parameters", () => {
    vi.stubGlobal("window", { location: { origin: "http://[::1]:8080" } });
    expect(socketUrl("/socket?mode=test&token=old#fragment", "a token")).toBe(
      "ws://[::1]:8080/socket?mode=test&token=a+token#fragment"
    );
  });

  it("creates a direct room for a participant", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch")
      .mockResolvedValueOnce(new Response(JSON.stringify({
        participant_credential: "credential-1",
        participant_id: "research-1"
      }), { status: 200, headers: { "Content-Type": "application/json" } }))
      .mockResolvedValueOnce(new Response(
        JSON.stringify({
          public_session_id: "ROOM1",
          role: "A",
          available_actions: null
        }),
        { status: 200, headers: { "Content-Type": "application/json" } }
      )
    );
    const client = new ParticipantClient({ baseUrl: "http://server.test" });

    await client.register();
    const room = await client.join();

    expect(room.sessionId).toBe("ROOM1");
    expect(room.availableActions).toBeNull();
    expect(fetchMock).toHaveBeenLastCalledWith("http://server.test/api/sessions", {
      method: "POST",
      headers: { "Content-Type": "application/json", Authorization: "Bearer credential-1" },
      body: JSON.stringify({})
    });
  });

  it("runs the supported experiment, consent, audio, and game-plan lifecycle", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch")
      .mockResolvedValueOnce(new Response(JSON.stringify({
        game_name: "Tiny Game",
        experiment_status: "active",
        consents: [],
        voice: { enabled: true }
      }), { status: 200 }))
      .mockResolvedValueOnce(participantResponse("one", "credential-1"))
      .mockResolvedValueOnce(new Response(null, { status: 204 }))
      .mockResolvedValueOnce(new Response(JSON.stringify({
        enabled: true,
        websocket_url: "/ws/audio/ROOM1",
        token: "audio-ticket",
        protocol_version: 1,
        sample_rate_hz: 24_000,
        channels: 1,
        frame_duration_ms: 20,
        jitter_buffer_ms: 100
      }), { status: 200 }))
      .mockResolvedValueOnce(new Response(JSON.stringify({
        websocket_url: "/ws/game/ROOM1",
        token: "ticket"
      }), { status: 200 }));
    const client = new ParticipantClient({ baseUrl: "http://server.test" });

    await expect(client.getExperiment()).resolves.toMatchObject({ gameName: "Tiny Game", status: "active" });
    await client.register();
    await client.acceptConsents({ research: true });
    await expect(client.getAudioSession("ROOM1")).resolves.toEqual({
      enabled: true,
      websocketUrl: "/ws/audio/ROOM1",
      token: "audio-ticket",
      protocolVersion: 1,
      sampleRateHz: 24_000,
      channels: 1,
      frameDurationMs: 20,
      jitterBufferMs: 100
    });
    await expect(client.getGameSession("ROOM1")).resolves.toEqual({
      websocketUrl: "/ws/game/ROOM1",
      token: "ticket"
    });

    expect(fetchMock).toHaveBeenNthCalledWith(3, "http://server.test/api/consent", {
      method: "POST",
      headers: { "Content-Type": "application/json", Authorization: "Bearer credential-1" },
      body: JSON.stringify({ decisions: { research: true } })
    });
  });

  it("sends an explicit leave message only on an open game channel", () => {
    vi.stubGlobal("WebSocket", { OPEN: 1 });
    const client = new ParticipantClient({ baseUrl: "http://server.test" });
    const send = vi.fn();

    client.leaveSession({ readyState: 1, send } as unknown as WebSocket);
    client.leaveSession({ readyState: 3, send } as unknown as WebSocket);

    expect(send).toHaveBeenCalledTimes(1);
    expect(send).toHaveBeenCalledWith(JSON.stringify({ type: "leave" }));
  });

  it("does not throw when an open socket closes during an explicit leave", () => {
    vi.stubGlobal("WebSocket", { OPEN: 1 });
    const client = new ParticipantClient({ baseUrl: "http://server.test" });
    const send = vi.fn(() => { throw new Error("closed concurrently"); });

    expect(() => client.leaveSession({ readyState: 1, send } as unknown as WebSocket)).not.toThrow();
    expect(send).toHaveBeenCalledOnce();
  });

  it("drops actions and chat on non-open or concurrently closing sockets", () => {
    vi.stubGlobal("WebSocket", { OPEN: 1 });
    const client = new ParticipantClient({ baseUrl: "http://server.test" });
    const closedSend = vi.fn();
    const throwingSend = vi.fn(() => { throw new Error("closed concurrently"); });

    expect(() => client.sendAction({ readyState: 3, send: closedSend } as unknown as WebSocket, {})).not.toThrow();
    expect(() => client.sendMessage({ readyState: 1, send: throwingSend } as unknown as WebSocket, "hi")).not.toThrow();
    expect(closedSend).not.toHaveBeenCalled();
    expect(throwingSend).toHaveBeenCalledOnce();
  });

  it("keeps the newest participant credential when create requests resolve out of order", async () => {
    const old = deferredResponse();
    const current = deferredResponse();
    vi.spyOn(globalThis, "fetch")
      .mockImplementationOnce(() => old.promise)
      .mockImplementationOnce(() => current.promise)
      .mockResolvedValueOnce(new Response(JSON.stringify({ public_session_id: "room", role: "A", available_actions: null }), { status: 200 }));
    const client = new ParticipantClient({ baseUrl: "http://server.test" });

    const oldRequest = client.register();
    const currentRequest = client.register();
    current.resolve(participantResponse("new", "new-credential"));
    await currentRequest;
    old.resolve(participantResponse("old", "old-credential"));
    await oldRequest;
    await client.join();

    expect(fetch).toHaveBeenLastCalledWith("http://server.test/api/sessions", expect.objectContaining({
      headers: expect.objectContaining({ Authorization: "Bearer new-credential" })
    }));
  });

  it("rejects authenticated calls before participant creation", async () => {
    const client = new ParticipantClient({ baseUrl: "http://server.test" });
    await expect(client.join()).rejects.toThrow("No participant credential");
  });

  it("retains an established credential when a later registration fails", async () => {
    vi.spyOn(globalThis, "fetch")
      .mockResolvedValueOnce(participantResponse("stable", "stable-credential"))
      .mockRejectedValueOnce(new TypeError("network unavailable"))
      .mockResolvedValueOnce(new Response(JSON.stringify({ public_session_id: "room", role: "A", available_actions: null }), { status: 200 }));
    const client = new ParticipantClient({ baseUrl: "http://server.test" });

    await client.register();
    await expect(client.register()).rejects.toThrow("network unavailable");
    await client.join();

    expect(fetch).toHaveBeenLastCalledWith("http://server.test/api/sessions", expect.objectContaining({
      headers: expect.objectContaining({ Authorization: "Bearer stable-credential" })
    }));
  });

  it("swallows synchronous and asynchronous voice-diagnostic failures", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch")
      .mockResolvedValueOnce(participantResponse("one", "credential"))
      .mockRejectedValueOnce(new TypeError("offline"));
    const client = new ParticipantClient({ baseUrl: "http://server.test" });
    await client.register();
    const cyclic: Record<string, unknown> = {};
    cyclic.self = cyclic;

    expect(() => client.postVoiceDiagnostic("ROOM1", "cyclic", cyclic)).not.toThrow();
    expect(() => client.postVoiceDiagnostic("ROOM1", "offline")).not.toThrow();
    await Promise.resolve();
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });
});

describe("checkedJson", () => {
  it("accepts JSON, empty success, and no-content responses", async () => {
    await expect(checkedJson(new Response("false", { status: 200 }))).resolves.toBe(false);
    await expect(checkedJson(new Response(null, { status: 204 }))).resolves.toBeUndefined();
    await expect(checkedJson(new Response("", { status: 200 }))).resolves.toBeUndefined();
  });

  it("preserves server error text and supplies a status fallback", async () => {
    await expect(checkedJson(new Response("capacity reached", { status: 429 }))).rejects.toThrow("capacity reached");
    await expect(checkedJson(new Response("", { status: 503 }))).rejects.toThrow("503");
  });

  it("reports malformed success JSON with its HTTP status", async () => {
    await expect(checkedJson(new Response("not json", { status: 200 }))).rejects.toThrow("invalid JSON (200)");
  });

  it("propagates network rejection without rewriting its cause", async () => {
    const failure = new TypeError("network unavailable");
    await expect(checkedJson(Promise.reject(failure))).rejects.toBe(failure);
  });
});

describe("shared participant protocol fixtures", () => {
  it("covers every client operation and server payload with the current public session identifier", () => {
    expect(fixtures.client_messages.map((message) => message.type)).toEqual([
      "ready",
      "action",
      "message",
      "heartbeat",
      "leave"
    ]);
    const messages = fixtures.server_messages as ServerMessage[];
    expect(messages.map((message) => message.type)).toEqual([
      "session_started",
      "transition",
      "message",
      "presence",
      "voice_status",
      "completed",
      "abandoned",
      "action_rejected",
      "error"
    ]);
    for (const message of messages) {
      expect(message.public_session_id).toBe("SESSION1");
      expect(message).not.toHaveProperty("room_id");
      expect(message).not.toHaveProperty("participant_session_id");
    }
  });

  it("maps the shared voice-transcript message onto the public camel-case value", () => {
    const fixture = fixtures.server_messages.find((message) => message.type === "message");
    if (!fixture || fixture.type !== "message") throw new Error("missing message fixture");
    expect(playerMessage(fixture.message)).toEqual({
      id: "message-1",
      sender: "B",
      text: "Hello",
      input: "voice_transcript",
      createdAt: "2026-08-21T12:00:00Z"
    });
  });
});
