import { afterEach, describe, expect, it, vi } from "vitest";
import { ExperimentApiClient, socketUrl } from "./protocol";

describe("ExperimentApiClient room helpers", () => {
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

  it("creates a direct room for a participant", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch")
      .mockResolvedValueOnce(new Response(JSON.stringify({
        participant_session_id: "participant-1",
        participant_credential: "credential-1",
        source: "direct"
      }), { status: 200, headers: { "Content-Type": "application/json" } }))
      .mockResolvedValueOnce(new Response(
        JSON.stringify({
          room_id: "ROOM1",
          participant_session_id: "participant-1",
          role: "A"
        }),
        { status: 200, headers: { "Content-Type": "application/json" } }
      )
    );
    const client = new ExperimentApiClient("http://server.test");

    const participant = await client.createParticipant();
    const room = await client.createRoom();

    expect(room.room_id).toBe("ROOM1");
    expect(fetchMock).toHaveBeenLastCalledWith("http://server.test/api/rooms", {
      method: "POST",
      headers: { "Content-Type": "application/json", Authorization: "Bearer credential-1" },
      body: JSON.stringify({})
    });
  });
});
