// @vitest-environment happy-dom

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { initialVoicePreflight, initialVoiceStatus, type AudioSessionSnapshot } from "./audio/types";
import type { PublicConfigResponse, RoomResponse } from "./protocol";
import { ParlandoStartupGate, type ActiveParlandoSession } from "./startup";

class FakeWebSocket extends EventTarget {
  static CONNECTING = 0;
  static OPEN = 1;
  static CLOSING = 2;
  static CLOSED = 3;
  static instances: FakeWebSocket[] = [];
  readyState = FakeWebSocket.CONNECTING;
  sent: string[] = [];
  close = vi.fn((code?: number, reason?: string) => {
    this.readyState = FakeWebSocket.CLOSED;
    this.dispatchEvent(new CloseEvent("close", { code, reason }));
  });

  /** Records each constructed game channel for deterministic event delivery. */
  constructor(readonly url: string) {
    super();
    FakeWebSocket.instances.push(this);
  }

  /** Records an outbound client control message. */
  send(data: string): void {
    this.sent.push(data);
  }

  /** Moves the transport to OPEN and emits its browser event. */
  open(): void {
    this.readyState = FakeWebSocket.OPEN;
    this.dispatchEvent(new Event("open"));
  }

  /** Delivers one JSON server message through the browser event contract. */
  message(value: unknown): void {
    this.dispatchEvent(new MessageEvent("message", { data: typeof value === "string" ? value : JSON.stringify(value) }));
  }
}

class FakeAudioController {
  disconnect = vi.fn(async () => undefined);
  prepare = vi.fn(async () => undefined);
  toggle = vi.fn(async () => undefined);
  updateVoiceStatus = vi.fn();
  private snapshotValue: AudioSessionSnapshot = {
    voiceStatus: initialVoiceStatus,
    voicePreflight: initialVoicePreflight
  };

  /** Returns the current startup audio state. */
  snapshot(): AudioSessionSnapshot {
    return this.snapshotValue;
  }

  /** Immediately publishes the initial state and returns a no-op cleanup. */
  subscribe(listener: (snapshot: AudioSessionSnapshot) => void): () => void {
    listener(this.snapshotValue);
    return () => undefined;
  }
}

/** Creates a public config suitable for rendered startup tests. */
function config(overrides: Partial<PublicConfigResponse> = {}): PublicConfigResponse {
  return {
    study_name: "Rendered Study",
    experiment_status: "active",
    consents: [],
    voice: { enabled: false },
    ...overrides
  };
}

/** Creates an API double while retaining Vitest call-order metadata. */
function api(publicConfig: PublicConfigResponse = config()) {
  const room: RoomResponse<{ turn: number }, { view: string }, { type: string }, { type: string }> = {
    room_id: "ROOM1",
    participant_session_id: "participant-1",
    role: "A",
    state: { turn: 0 },
    observation: { view: "initial" }
  };
  return {
    getPublicConfig: vi.fn(async () => publicConfig),
    createParticipant: vi.fn(async () => ({ participant_session_id: "participant-1", participant_credential: "credential", participant_id: "research", source: "direct" as const })),
    submitConsent: vi.fn(async () => undefined),
    createRoom: vi.fn(async () => room),
    getGameSession: vi.fn(async () => ({ websocket_url: "/ws/game/ROOM1", token: "ticket" })),
    socketUrl: vi.fn(() => "ws://study.test/ws/game/ROOM1?token=ticket"),
    sendAction: vi.fn(),
    sendChatMessage: vi.fn(),
    leaveSession: vi.fn(),
    postVoiceDiagnostic: vi.fn(),
    getAudioSession: vi.fn()
  };
}

/** Renders a compact active-session projection for transport assertions. */
function game(session: ActiveParlandoSession<{ turn: number }, { view: string }, { type: string }, { type: string }>) {
  return (
    <div>
      <span>active:{session.connected ? "connected" : "disconnected"}</span>
      <span>observation:{session.observation?.view}</span>
      <span>completed:{String(session.completed)}</span>
      {session.completionSummary && <span>summary</span>}
      <button onClick={() => session.sendAction({ type: "move" })}>Move</button>
    </div>
  );
}

beforeEach(() => {
  FakeWebSocket.instances = [];
  vi.stubGlobal("WebSocket", FakeWebSocket);
  Object.defineProperty(navigator, "mediaDevices", {
    configurable: true,
    value: { enumerateDevices: vi.fn(async () => []), addEventListener: vi.fn(), removeEventListener: vi.fn() }
  });
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("ParlandoStartupGate rendered state machine", () => {
  it("shows loading and then a closed-intake explanation", async () => {
    const client = api(config({ experiment_status: "inactive" }));
    render(<ParlandoStartupGate apiClient={client as never} createAudioController={() => new FakeAudioController() as never} renderGame={game} />);
    expect(screen.getByRole("heading", { name: "Loading study" })).toBeInTheDocument();
    expect(await screen.findByRole("heading", { name: "Experiment not accepting participants" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Enter waiting room" })).not.toBeInTheDocument();
  });

  it("gates required consent and performs create-consent-room in order once", async () => {
    const client = api(config({
      participant_information_url: "https://example.test/info",
      participant_information_version: "v2",
      consents: [{ id: "study", title: "Study consent", body: "I agree", required: true }]
    }));
    render(<ParlandoStartupGate apiClient={client as never} createAudioController={() => new FakeAudioController() as never} renderGame={game} />);
    const enter = await screen.findByRole("button", { name: "Enter waiting room" });
    expect(enter).toBeDisabled();
    fireEvent.click(screen.getByRole("checkbox", { name: /Study consent/ }));
    expect(enter).toBeEnabled();
    fireEvent.click(enter);
    fireEvent.click(enter);

    await waitFor(() => expect(FakeWebSocket.instances).toHaveLength(1));
    expect(client.createParticipant).toHaveBeenCalledOnce();
    expect(client.submitConsent).toHaveBeenCalledWith({ study: true });
    expect(client.createRoom).toHaveBeenCalledOnce();
    expect(client.createParticipant.mock.invocationCallOrder[0]).toBeLessThan(client.submitConsent.mock.invocationCallOrder[0]);
    expect(client.submitConsent.mock.invocationCallOrder[0]).toBeLessThan(client.createRoom.mock.invocationCallOrder[0]);
  });

  it("activates on role assignment, applies completion, and blocks late actions", async () => {
    const client = api();
    render(<ParlandoStartupGate apiClient={client as never} createAudioController={() => new FakeAudioController() as never} renderGame={game} />);
    fireEvent.click(await screen.findByRole("button", { name: "Enter waiting room" }));
    await waitFor(() => expect(FakeWebSocket.instances).toHaveLength(1));
    const socket = FakeWebSocket.instances[0];
    act(() => socket.open());
    expect(socket.sent).toContain(JSON.stringify({ type: "ready" }));
    act(() => socket.message({
      type: "roleAssigned",
      room_id: "ROOM1",
      participant_session_id: "participant-1",
      role: "A",
      observation: { view: "assigned" }
    }));
    expect(screen.getByText("active:connected")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Move" }));
    expect(client.sendAction).toHaveBeenCalledOnce();

    act(() => socket.message({ type: "completed", room_id: "ROOM1", summary: { outcome: "win" } }));
    expect(screen.getByText("completed:true")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Move" }));
    expect(client.sendAction).toHaveBeenCalledOnce();
  });

  it("closes malformed server messages with a protocol error", async () => {
    const client = api();
    render(<ParlandoStartupGate apiClient={client as never} createAudioController={() => new FakeAudioController() as never} renderGame={game} />);
    fireEvent.click(await screen.findByRole("button", { name: "Enter waiting room" }));
    await waitFor(() => expect(FakeWebSocket.instances).toHaveLength(1));
    const socket = FakeWebSocket.instances[0];
    act(() => socket.open());
    act(() => socket.message("{invalid"));
    expect(socket.close).toHaveBeenCalledWith(1002, "Invalid server message");
    expect(await screen.findByText("The server sent an invalid game message.")).toBeInTheDocument();
  });

  it("ignores callbacks from a socket replaced during reconnect", async () => {
    vi.useFakeTimers();
    const client = api();
    render(<ParlandoStartupGate apiClient={client as never} createAudioController={() => new FakeAudioController() as never} renderGame={game} />);
    await act(async () => {
      await vi.runAllTicks();
    });
    fireEvent.click(screen.getByRole("button", { name: "Enter waiting room" }));
    await act(async () => {
      await vi.runAllTicks();
    });
    const oldSocket = FakeWebSocket.instances[0];
    act(() => oldSocket.open());
    act(() => oldSocket.dispatchEvent(new CloseEvent("close")));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1_000);
    });
    expect(FakeWebSocket.instances).toHaveLength(2);
    const currentSocket = FakeWebSocket.instances[1];
    act(() => currentSocket.open());
    act(() => currentSocket.message({
      type: "roleAssigned",
      room_id: "ROOM1",
      participant_session_id: "participant-1",
      role: "A",
      observation: { view: "current" }
    }));

    act(() => oldSocket.dispatchEvent(new Event("error")));
    act(() => oldSocket.message({ type: "error", message: "stale error" }));
    expect(screen.queryByText(/stale error|Retrying/)).not.toBeInTheDocument();
    expect(screen.getByText("observation:current")).toBeInTheDocument();
  });
});
