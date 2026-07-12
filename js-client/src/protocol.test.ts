import { afterEach, describe, expect, it, vi } from "vitest";
import { ExperimentApiClient } from "./protocol";

describe("ExperimentApiClient room helpers", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("creates a direct room for a participant", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(
        JSON.stringify({
          room_id: "ROOM1",
          participant_session_id: "participant-1",
          role: "A"
        }),
        { status: 200, headers: { "Content-Type": "application/json" } }
      )
    );
    const client = new ExperimentApiClient("http://server.test");

    const room = await client.createRoom("participant-1");

    expect(room.room_id).toBe("ROOM1");
    expect(fetchMock).toHaveBeenCalledWith("http://server.test/api/rooms", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ participant_session_id: "participant-1", mode: "direct" })
    });
  });

  it("joins an existing room for a participant", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch").mockResolvedValue(
      new Response(
        JSON.stringify({
          room_id: "ROOM1",
          participant_session_id: "participant-2",
          role: "B"
        }),
        { status: 200, headers: { "Content-Type": "application/json" } }
      )
    );
    const client = new ExperimentApiClient("http://server.test");

    const room = await client.joinRoom("ROOM1", "participant-2");

    expect(room.role).toBe("B");
    expect(fetchMock).toHaveBeenCalledWith("http://server.test/api/rooms/ROOM1/join", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ participant_session_id: "participant-2" })
    });
  });
});
