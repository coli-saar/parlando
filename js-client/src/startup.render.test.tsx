// @vitest-environment happy-dom

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { initialVoicePreflight, initialVoiceStatus, type AudioSessionSnapshot } from "./audio/types";
import type { ExperimentInfo, JoinedSession } from "./protocol";
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
    gameName: "Tiny Game",
    status: "active",
    consents: [],
    voice: { enabled: false },
    ...overrides
  };
}

/** Creates an API double while retaining Vitest call-order metadata. */
function api(publicConfig: ExperimentInfo = config()) {
  const room: JoinedSession<{ view: string }, { type: string }> = {
    sessionId: "ROOM1",
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
      <span>conversation:{session.conversation.length}</span>
      <span>presence:{String(Boolean(session.presence.A?.connected))}:{String(Boolean(session.presence.B?.connected))}</span>
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
    expect(screen.getByText("Tiny Game")).toBeInTheDocument();
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
      public_session_id: "ROOM1",
      role: "A",
      observation: { view: "assigned" },
      available_actions: null
    }));
    expect(screen.getByText("active:connected")).toBeInTheDocument();
    act(() => socket.message({
      protocol_version: 1,
      type: "transition",
      public_session_id: "ROOM1",
      actor: "B",
      action: { type: "move" },
      observation: { view: "moved" },
      available_actions: []
    }));
    expect(screen.getByText("observation:moved")).toBeInTheDocument();
    expect(screen.getByText("transition:B:move")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Move" }));
    expect(client.sendAction).toHaveBeenCalledOnce();

    act(() => socket.message({ protocol_version: 1, type: "completed", public_session_id: "ROOM1", completion: { outcome: "win" } }));
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
    act(() => {
      oldSocket.readyState = FakeWebSocket.CLOSED;
      oldSocket.dispatchEvent(new CloseEvent("close"));
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1_000);
    });
    expect(FakeWebSocket.instances).toHaveLength(2);
    const currentSocket = FakeWebSocket.instances[1];
    act(() => currentSocket.open());
    act(() => currentSocket.message({
      protocol_version: 1,
      type: "session_started",
      public_session_id: "ROOM1",
      role: "A",
      observation: { view: "current" },
      available_actions: null
    }));

    act(() => oldSocket.dispatchEvent(new Event("error")));
    act(() => oldSocket.message({ protocol_version: 1, type: "error", public_session_id: "ROOM1", code: "stale_error", fatal: false }));
    expect(screen.queryByText(/stale error|Retrying/)).not.toBeInTheDocument();
    expect(screen.getByText("observation:current")).toBeInTheDocument();
  });

  it("moves the heartbeat to the current socket and stops it on unmount", async () => {
    vi.useFakeTimers();
    const client = api();
    const rendered = render(<ParticipantAppTestHarness apiClient={client as never} createAudioController={() => new FakeAudioController() as never} renderGame={game} />);
    await act(async () => {
      await vi.runAllTicks();
    });
    fireEvent.click(screen.getByRole("button", { name: "Enter waiting room" }));
    await act(async () => {
      await vi.runAllTicks();
    });
    const oldSocket = FakeWebSocket.instances[0];
    act(() => oldSocket.open());
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1_000);
    });
    expect(oldSocket.sent.filter((message) => message === JSON.stringify({ type: "heartbeat" }))).toHaveLength(1);

    act(() => {
      oldSocket.readyState = FakeWebSocket.CLOSED;
      oldSocket.dispatchEvent(new CloseEvent("close"));
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1_000);
    });
    const currentSocket = FakeWebSocket.instances[1];
    act(() => currentSocket.open());
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1_000);
    });
    expect(oldSocket.sent.filter((message) => message === JSON.stringify({ type: "heartbeat" }))).toHaveLength(1);
    expect(currentSocket.sent).toContain(JSON.stringify({ type: "heartbeat" }));

    const sentBeforeUnmount = currentSocket.sent.length;
    rendered.unmount();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(5_000);
    });
    expect(currentSocket.sent).toHaveLength(sentBeforeUnmount);
  });

  it("uses one, then two seconds for consecutive reconnect failures", async () => {
    vi.useFakeTimers();
    const client = api();
    client.getGameSession
      .mockResolvedValueOnce({ websocketUrl: "/ws/game/ROOM1", token: "first" })
      .mockRejectedValueOnce(new Error("ticket unavailable"))
      .mockResolvedValueOnce({ websocketUrl: "/ws/game/ROOM1", token: "third" });
    render(<ParticipantAppTestHarness apiClient={client as never} createAudioController={() => new FakeAudioController() as never} renderGame={game} />);
    await act(async () => {
      await vi.runAllTicks();
    });
    fireEvent.click(screen.getByRole("button", { name: "Enter waiting room" }));
    await act(async () => {
      await vi.runAllTicks();
    });
    const firstSocket = FakeWebSocket.instances[0];
    act(() => firstSocket.open());
    act(() => {
      firstSocket.readyState = FakeWebSocket.CLOSED;
      firstSocket.dispatchEvent(new CloseEvent("close"));
    });

    await act(async () => {
      await vi.advanceTimersByTimeAsync(999);
    });
    expect(client.getGameSession).toHaveBeenCalledTimes(1);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1);
    });
    expect(client.getGameSession).toHaveBeenCalledTimes(2);
    expect(FakeWebSocket.instances).toHaveLength(1);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(1_999);
    });
    expect(client.getGameSession).toHaveBeenCalledTimes(2);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1);
    });
    expect(client.getGameSession).toHaveBeenCalledTimes(3);
    expect(FakeWebSocket.instances).toHaveLength(2);
  });

  it("normalizes presence and deduplicates a bounded conversation", async () => {
    const client = api();
    const audio = new FakeAudioController();
    render(<ParticipantAppTestHarness apiClient={client as never} createAudioController={() => audio as never} renderGame={game} />);
    fireEvent.click(await screen.findByRole("button", { name: "Enter waiting room" }));
    await waitFor(() => expect(FakeWebSocket.instances).toHaveLength(1));
    const socket = FakeWebSocket.instances[0];
    act(() => socket.open());
    act(() => socket.message({
      protocol_version: 1,
      type: "session_started",
      public_session_id: "ROOM1",
      role: "A",
      observation: { view: "active" },
      available_actions: null
    }));
    act(() => socket.message({
      protocol_version: 1,
      type: "presence",
      public_session_id: "ROOM1",
      presence: { A: { connected: 1, private: "hidden" }, B: { connected: true } }
    }));
    expect(screen.getByText("presence:true:true")).toBeInTheDocument();

    for (let index = 0; index < 55; index += 1) {
      act(() => socket.message({
        protocol_version: 1,
        type: "message",
        public_session_id: "ROOM1",
        message: {
          id: `message-${index}`,
          sender: "A",
          text: String(index),
          input: "text",
          created_at: "2026-01-01T00:00:00Z"
        }
      }));
    }
    act(() => socket.message({
      protocol_version: 1,
      type: "message",
      public_session_id: "ROOM1",
      message: {
        id: "message-54",
        sender: "A",
        text: "duplicate",
        input: "text",
        created_at: "2026-01-01T00:00:00Z"
      }
    }));
    expect(screen.getByText("conversation:50")).toBeInTheDocument();

    act(() => socket.message({
      protocol_version: 1,
      type: "voice_status",
      public_session_id: "ROOM1",
      voice: { transcriptionReady: true, transcriptionStatus: "ready" }
    }));
    expect(audio.updateVoiceStatus).toHaveBeenCalledWith({
      transcriptionReady: true,
      transcriptionMessage: "ready"
    });
    act(() => socket.message({
      protocol_version: 1,
      type: "action_rejected",
      public_session_id: "ROOM1",
      code: "invalid_action"
    }));
    expect(screen.getByText("Action rejected: invalid_action")).toBeInTheDocument();
  });

  it("abandons the current generation, disconnects voice, and rejects stale callbacks", async () => {
    const client = api();
    const audio = new FakeAudioController();
    render(<ParticipantAppTestHarness apiClient={client as never} createAudioController={() => audio as never} renderGame={game} />);
    fireEvent.click(await screen.findByRole("button", { name: "Enter waiting room" }));
    await waitFor(() => expect(FakeWebSocket.instances).toHaveLength(1));
    const socket = FakeWebSocket.instances[0];
    act(() => socket.open());
    act(() => socket.message({
      protocol_version: 1,
      type: "session_started",
      public_session_id: "ROOM1",
      role: "A",
      observation: { view: "active" },
      available_actions: null
    }));
    act(() => socket.message({
      protocol_version: 1,
      type: "abandoned",
      public_session_id: "ROOM1",
      code: "participant_left"
    }));

    expect(audio.disconnect).toHaveBeenCalledWith(true);
    expect(socket.close).toHaveBeenCalledOnce();
    expect(await screen.findByText("This session ended because a player left.")).toBeInTheDocument();
    expect(screen.queryByText("observation:active")).not.toBeInTheDocument();
    act(() => socket.message({ protocol_version: 1, type: "error", public_session_id: "ROOM1", code: "stale", fatal: true }));
    expect(screen.queryByText("The server rejected the last request.")).not.toBeInTheDocument();
  });

  it("browser teardown closes one owning socket and cancels heartbeats", async () => {
    vi.useFakeTimers();
    const client = api();
    const rendered = render(<ParticipantAppTestHarness apiClient={client as never} createAudioController={() => new FakeAudioController() as never} renderGame={game} />);
    await act(async () => {
      await vi.runAllTicks();
    });
    fireEvent.click(screen.getByRole("button", { name: "Enter waiting room" }));
    await act(async () => {
      await vi.runAllTicks();
    });
    const socket = FakeWebSocket.instances[0];
    act(() => socket.open());
    act(() => window.dispatchEvent(new PageTransitionEvent("pagehide")));
    expect(socket.close).toHaveBeenCalledOnce();
    expect(client.leaveSession).not.toHaveBeenCalled();

    rendered.unmount();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(5_000);
    });
    expect(socket.close).toHaveBeenCalledOnce();
  });

  it("stops reconnecting at the exact five-minute window", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-08-21T12:00:00Z"));
    const client = api();
    client.getGameSession
      .mockResolvedValueOnce({ websocketUrl: "/ws/game/ROOM1", token: "first" })
      .mockRejectedValue(new Error("ticket unavailable"));
    render(<ParticipantAppTestHarness apiClient={client as never} createAudioController={() => new FakeAudioController() as never} renderGame={game} />);
    await act(async () => {
      await vi.runAllTicks();
    });
    fireEvent.click(screen.getByRole("button", { name: "Enter waiting room" }));
    await act(async () => {
      await vi.runAllTicks();
    });
    const socket = FakeWebSocket.instances[0];
    act(() => socket.open());
    act(() => {
      socket.readyState = FakeWebSocket.CLOSED;
      socket.dispatchEvent(new CloseEvent("close"));
    });

    await act(async () => {
      await vi.advanceTimersByTimeAsync(5 * 60_000);
    });
    expect(screen.getByText("The five-minute reconnection window expired. Please leave and start a new session.")).toBeInTheDocument();
    const callsAtExpiry = client.getGameSession.mock.calls.length;
    await act(async () => {
      await vi.advanceTimersByTimeAsync(60_000);
    });
    expect(client.getGameSession).toHaveBeenCalledTimes(callsAtExpiry);
  });
});
