// @vitest-environment happy-dom

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { initialVoicePreflight, initialVoiceStatus, type AudioSessionSnapshot } from "./audio/types";
import type { ExperimentInfo, JoinedRoom } from "./protocol";
import { ParticipantAppTestHarness, type GameSession } from "./startup";

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
  connect = vi.fn(async () => undefined);
  disconnect = vi.fn(async () => undefined);
  prepare = vi.fn(async () => undefined);
  setMicrophoneMuted = vi.fn(async () => undefined);
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
function config(overrides: Partial<ExperimentInfo> = {}): ExperimentInfo {
  return {
    status: "active",
    consents: [],
    voice: { enabled: false },
    ...overrides
  };
}

/** Creates an API double while retaining Vitest call-order metadata. */
function api(publicConfig: ExperimentInfo = config()) {
  const room: JoinedRoom<{ view: string }, { type: string }> = {
    roomId: "ROOM1",
    role: "A",
    observation: { view: "initial" },
    availableActions: null,
    presence: { A: { connected: false } }
  };
  return {
    getExperiment: vi.fn(async () => publicConfig),
    register: vi.fn(async () => undefined),
    acceptConsents: vi.fn(async () => undefined),
    join: vi.fn(async () => room),
    getGameSession: vi.fn(async () => ({ websocketUrl: "/ws/game/ROOM1", token: "ticket" })),
    socketUrl: vi.fn(() => "ws://study.test/ws/game/ROOM1?token=ticket"),
    sendAction: vi.fn(),
    message: vi.fn(),
    leaveSession: vi.fn(),
    postVoiceDiagnostic: vi.fn(),
    getAudioSession: vi.fn()
  };
}

/** Renders a compact active-session projection for transport assertions. */
function game(session: GameSession<{ view: string }, { type: string }, { type: string }>) {
  return (
    <div>
      <span>active:{session.connected ? "connected" : "disconnected"}</span>
      <span>observation:{session.observation?.view}</span>
      {session.transition && (
        <span>transition:{session.transition.actor}:{session.transition.action.type}</span>
      )}
      <span>completed:{String(session.completed)}</span>
      {session.completion && <span>completion</span>}
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

describe("ParticipantApp rendered state machine", () => {
  it("shows loading and then a closed-intake explanation", async () => {
    const client = api(config({ status: "inactive" }));
    render(<ParticipantAppTestHarness apiClient={client as never} createAudioController={() => new FakeAudioController() as never} renderGame={game} />);
    expect(screen.getByRole("heading", { name: "Loading experiment" })).toBeInTheDocument();
    expect(await screen.findByRole("heading", { name: "Experiment not accepting participants" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Enter waiting room" })).not.toBeInTheDocument();
  });

  it("gates required consent and performs create-consent-room in order once", async () => {
    const client = api(config({
      participantInformationUrl: "https://example.test/info",
      participantInformationVersion: "v2",
      consents: [{ id: "study", title: "Study consent", body: "I agree", required: true }]
    }));
    render(<ParticipantAppTestHarness apiClient={client as never} createAudioController={() => new FakeAudioController() as never} renderGame={game} />);
    const enter = await screen.findByRole("button", { name: "Enter waiting room" });
    expect(enter).toBeDisabled();
    fireEvent.click(screen.getByRole("checkbox", { name: /Study consent/ }));
    expect(enter).toBeEnabled();
    fireEvent.click(enter);
    fireEvent.click(enter);

    await waitFor(() => expect(FakeWebSocket.instances).toHaveLength(1));
    expect(client.register).toHaveBeenCalledOnce();
    expect(client.acceptConsents).toHaveBeenCalledWith({ study: true });
    expect(client.join).toHaveBeenCalledOnce();
    expect(client.register.mock.invocationCallOrder[0]).toBeLessThan(client.acceptConsents.mock.invocationCallOrder[0]);
    expect(client.acceptConsents.mock.invocationCallOrder[0]).toBeLessThan(client.join.mock.invocationCallOrder[0]);
  });

  it("activates on role assignment, applies completion, and blocks late actions", async () => {
    const client = api();
    render(<ParticipantAppTestHarness apiClient={client as never} createAudioController={() => new FakeAudioController() as never} renderGame={game} />);
    fireEvent.click(await screen.findByRole("button", { name: "Enter waiting room" }));
    await waitFor(() => expect(FakeWebSocket.instances).toHaveLength(1));
    const socket = FakeWebSocket.instances[0];
    act(() => socket.open());
    expect(socket.sent).toContain(JSON.stringify({ type: "ready" }));
    act(() => socket.message({
      protocol_version: 1,
      type: "session_started",
      room_id: "ROOM1",
      role: "A",
      observation: { view: "assigned" },
      available_actions: null
    }));
    expect(screen.getByText("active:connected")).toBeInTheDocument();
    act(() => socket.message({
      protocol_version: 1,
      type: "transition",
      room_id: "ROOM1",
      actor: "B",
      action: { type: "move" },
      observation: { view: "moved" },
      available_actions: []
    }));
    expect(screen.getByText("observation:moved")).toBeInTheDocument();
    expect(screen.getByText("transition:B:move")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Move" }));
    expect(client.sendAction).toHaveBeenCalledOnce();

    act(() => socket.message({ protocol_version: 1, type: "completed", room_id: "ROOM1", completion: { outcome: "win" } }));
    expect(screen.getByText("completed:true")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Move" }));
    expect(client.sendAction).toHaveBeenCalledOnce();
  });

  it("closes malformed server messages with a protocol error", async () => {
    const client = api();
    render(<ParticipantAppTestHarness apiClient={client as never} createAudioController={() => new FakeAudioController() as never} renderGame={game} />);
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
    render(<ParticipantAppTestHarness apiClient={client as never} createAudioController={() => new FakeAudioController() as never} renderGame={game} />);
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
      protocol_version: 1,
      type: "session_started",
      room_id: "ROOM1",
      role: "A",
      observation: { view: "current" },
      available_actions: null
    }));

    act(() => oldSocket.dispatchEvent(new Event("error")));
    act(() => oldSocket.message({ protocol_version: 1, type: "error", room_id: "ROOM1", code: "stale_error", fatal: false }));
    expect(screen.queryByText(/stale error|Retrying/)).not.toBeInTheDocument();
    expect(screen.getByText("observation:current")).toBeInTheDocument();
  });
});
