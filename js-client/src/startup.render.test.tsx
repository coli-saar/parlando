// @vitest-environment happy-dom

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { initialVoicePreflight, initialVoiceStatus } from "./audio/types";
import type { ExperimentInfo, ParticipantState } from "./protocol";
import { ParticipantAppTestHarness, type GameSession } from "./startup";

class FakeWebSocket extends EventTarget {
  static OPEN = 1;
  static instances: FakeWebSocket[] = [];
  readyState = 0;
  sent: string[] = [];
  close = vi.fn(() => { this.readyState = 3; this.dispatchEvent(new Event("close")); });
  constructor(readonly url: string) { super(); FakeWebSocket.instances.push(this); }
  send(data: string): void { this.sent.push(data); }
  open(): void { this.readyState = 1; this.dispatchEvent(new Event("open")); }
  message(value: unknown): void { this.dispatchEvent(new MessageEvent("message", { data: JSON.stringify(value) })); }
}

const waiting: ParticipantState<{ view: string }, { type: string }, { score: number }> = {
  state: "waiting", public_session_id: "ROOM1", role: "A", presence: {}, waiting_started_at: "2099-01-01T00:00:00Z", waiting_deadline_at: "2099-01-01T00:10:00Z"
};
const active: ParticipantState<{ view: string }, { type: string }, { score: number }> = {
  state: "active", public_session_id: "ROOM1", role: "A", observation: { view: "live" }, available_actions: null, presence: {}, idle_deadline_at: "2099-01-01T00:00:00Z"
};

function config(): ExperimentInfo {
  return { gameName: "Tiny Game", status: "active", consents: [], voice: { enabled: false } };
}

function api() {
  return {
    getExperiment: vi.fn(async () => config()), register: vi.fn(async () => undefined),
    acceptConsents: vi.fn(async () => undefined), join: vi.fn(async () => waiting),
    getParticipantState: vi.fn(async () => active),
    getGameSession: vi.fn(async () => ({ websocketUrl: "/ws/game/ROOM1", token: "ticket" })),
    socketUrl: vi.fn(() => "ws://study.test/ws/game/ROOM1?token=ticket"),
    sendAction: vi.fn(), sendMessage: vi.fn(), postVoiceDiagnostic: vi.fn(), getAudioSession: vi.fn(),
    leaveSession: vi.fn(async () => ({
      state: "ended", public_session_id: "ROOM1", role: "A",
      result: { outcome: "left_game", reason: "participant_left", completion: null, final_observation: { view: "live" }, handoff: null }
    }))
  };
}

const audio = {
  disconnect: vi.fn(async () => undefined), prepare: vi.fn(async () => undefined), connect: vi.fn(async () => undefined),
  setMicrophoneMuted: vi.fn(async () => undefined), updateVoiceStatus: vi.fn(),
  snapshot: () => ({ voiceStatus: initialVoiceStatus, voicePreflight: initialVoicePreflight }),
  subscribe: (listener: (value: unknown) => void) => { listener({ voiceStatus: initialVoiceStatus, voicePreflight: initialVoicePreflight }); return () => undefined; }
};

function game(session: GameSession<{ view: string }, { type: string }, { score: number }>) {
  return <div><span>{session.observation.view}</span><span>{String(session.interactionEnabled)}</span><button onClick={session.leave}>Leave</button></div>;
}

beforeEach(() => { FakeWebSocket.instances = []; vi.stubGlobal("WebSocket", FakeWebSocket); Object.defineProperty(navigator, "mediaDevices", { configurable: true, value: { enumerateDevices: vi.fn(async () => []), addEventListener: vi.fn(), removeEventListener: vi.fn() } }); });
afterEach(() => { cleanup(); vi.unstubAllGlobals(); vi.restoreAllMocks(); });

describe("ParticipantApp participant state machine", () => {
  it("moves Waiting to Active only from an authoritative snapshot", async () => {
    const client = api();
    render(<ParticipantAppTestHarness apiClient={client as never} createAudioController={() => audio as never} renderGame={game} />);
    fireEvent.click(await screen.findByRole("button", { name: "Enter waiting room" }));
    await waitFor(() => expect(FakeWebSocket.instances).toHaveLength(1));
    const socket = FakeWebSocket.instances[0];
    act(() => socket.open());
    expect(screen.queryByText("live")).not.toBeInTheDocument();
    act(() => socket.message({ protocol_version: 2, type: "participant_state", participant_state: active }));
    expect(await screen.findByText("live")).toBeInTheDocument();
    expect(screen.getByText("true")).toBeInTheDocument();
  });

  it("renders Paused while strictly disabling interaction", async () => {
    const client = api();
    render(<ParticipantAppTestHarness apiClient={client as never} createAudioController={() => audio as never} renderGame={game} />);
    fireEvent.click(await screen.findByRole("button", { name: "Enter waiting room" }));
    await waitFor(() => expect(FakeWebSocket.instances).toHaveLength(1));
    const socket = FakeWebSocket.instances[0]; act(() => socket.open());
    act(() => socket.message({ protocol_version: 2, type: "participant_state", participant_state: active }));
    act(() => socket.message({ protocol_version: 2, type: "participant_state", participant_state: { ...active, state: "paused", reason: { type: "partner_reconnecting", deadline_at: "2030-01-01T00:00:00Z" } } }));
    expect(await screen.findByText("false")).toBeInTheDocument();
    expect(screen.getByText(/reconnect/i)).toBeInTheDocument();
  });

  it("waits for the HTTP terminal response before closing on leave", async () => {
    const client = api();
    render(<ParticipantAppTestHarness apiClient={client as never} createAudioController={() => audio as never} renderGame={game} />);
    fireEvent.click(await screen.findByRole("button", { name: "Enter waiting room" }));
    await waitFor(() => expect(FakeWebSocket.instances).toHaveLength(1));
    const socket = FakeWebSocket.instances[0]; act(() => { socket.open(); socket.message({ protocol_version: 2, type: "participant_state", participant_state: active }); });
    fireEvent.click(await screen.findByRole("button", { name: "Leave" }));
    await waitFor(() => expect(client.leaveSession).toHaveBeenCalledWith("ROOM1"));
    await waitFor(() => expect(socket.close).toHaveBeenCalled());
    expect(await screen.findByText(/left after the game started/i)).toBeInTheDocument();
  });
});
