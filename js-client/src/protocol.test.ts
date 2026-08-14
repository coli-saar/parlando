import { afterEach, describe, expect, it, vi } from "vitest";
import { ExperimentApiClient } from "./protocol";

describe("ExperimentApiClient room helpers", () => {
  afterEach(() => {
    vi.restoreAllMocks();
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
    const room = await client.createRoom(participant.participant_session_id);

    expect(room.room_id).toBe("ROOM1");
    expect(fetchMock).toHaveBeenLastCalledWith("http://server.test/api/rooms", {
      method: "POST",
      headers: { "Content-Type": "application/json", Authorization: "Bearer credential-1" },
      body: JSON.stringify({ participant_session_id: "participant-1", mode: "direct" })
    });
  });

  it("joins an existing room for a participant", async () => {
    const fetchMock = vi.spyOn(globalThis, "fetch")
      .mockResolvedValueOnce(new Response(JSON.stringify({
        participant_session_id: "participant-2",
        participant_credential: "credential-2",
        source: "direct"
      }), { status: 200, headers: { "Content-Type": "application/json" } }))
      .mockResolvedValueOnce(new Response(
        JSON.stringify({
          room_id: "ROOM1",
          participant_session_id: "participant-2",
          role: "B"
        }),
        { status: 200, headers: { "Content-Type": "application/json" } }
      )
    );
    const client = new ExperimentApiClient("http://server.test");

    const participant = await client.createParticipant();
    const room = await client.joinRoom("ROOM1", participant.participant_session_id);

    expect(room.role).toBe("B");
    expect(fetchMock).toHaveBeenLastCalledWith("http://server.test/api/rooms/ROOM1/join", {
      method: "POST",
      headers: { "Content-Type": "application/json", Authorization: "Bearer credential-2" },
      body: JSON.stringify({ participant_session_id: "participant-2" })
    });
  });
});
