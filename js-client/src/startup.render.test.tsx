// @vitest-environment happy-dom

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { initialVoicePreflight, initialVoiceStatus } from "./audio/types";
import type { ExperimentInfo, ParticipantOutcome, ParticipantState } from "./protocol";
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
    getExperiment: vi.fn(async () => config()), hasCredential: vi.fn(() => false), register: vi.fn(async () => undefined),
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
  return <div>
    <span>{session.observation.view}</span>
    <span>{String(session.interactionEnabled)}</span>
    <span>Messages: {session.conversation.length}</span>
    <span>Actor: {session.transition?.actor ?? "none"}</span>
    <span>Player B: {session.presence.B?.connected ? "connected" : "waiting"}</span>
    <button onClick={session.leave}>Leave</button>
  </div>;
}

beforeEach(() => { FakeWebSocket.instances = []; vi.stubGlobal("WebSocket", FakeWebSocket); Object.defineProperty(navigator, "mediaDevices", { configurable: true, value: { enumerateDevices: vi.fn(async () => []), addEventListener: vi.fn(), removeEventListener: vi.fn() } }); });
afterEach(() => { cleanup(); vi.unstubAllGlobals(); vi.restoreAllMocks(); });

describe("ParticipantApp participant state machine", () => {
  it("restores a completed participant directly to the terminal outcome", async () => {
    const client = api();
    client.hasCredential = vi.fn(() => true);
    client.join.mockResolvedValue({
      state: "ended",
      public_session_id: "ROOM1",
      role: "A",
      result: {
        outcome: "completed",
        reason: "game_completed",
        completion: null,
        final_observation: { view: "final" },
        handoff: null
      }
    });

    render(<ParticipantAppTestHarness apiClient={client as never} createAudioController={() => audio as never} renderGame={game} />);

    expect(await screen.findByText(/responses have been recorded/i)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Enter waiting room" })).not.toBeInTheDocument();
    expect(client.register).not.toHaveBeenCalled();
    expect(client.getGameSession).not.toHaveBeenCalled();
  });

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

  it("applies every live server update to the public game session", async () => {
    const client = api();
    render(<ParticipantAppTestHarness apiClient={client as never} createAudioController={() => audio as never} renderGame={game} />);
    fireEvent.click(await screen.findByRole("button", { name: "Enter waiting room" }));
    await waitFor(() => expect(FakeWebSocket.instances).toHaveLength(1));
    const socket = FakeWebSocket.instances[0];
    act(() => {
      socket.open();
      socket.message({ protocol_version: 2, type: "participant_state", participant_state: active });
      socket.message({
        protocol_version: 2,
        type: "transition",
        public_session_id: "ROOM1",
        actor: "B",
        action: { type: "move" },
        observation: { view: "moved" },
        available_actions: [{ type: "reply" }]
      });
      socket.message({ protocol_version: 2, type: "presence", public_session_id: "ROOM1", presence: { B: { connected: true, audioReady: false } } });
      socket.message({
        protocol_version: 2,
        type: "message",
        public_session_id: "ROOM1",
        message: { id: "m1", sender: "B", text: "hello", input: "text", created_at: "2026-08-31T12:00:00Z" }
      });
      socket.message({ protocol_version: 2, type: "voice_status", public_session_id: "ROOM1", voice: { transcriptionReady: true, transcriptionStatus: "Ready" } });
    });

    expect(await screen.findByText("moved")).toBeInTheDocument();
    expect(screen.getByText("Actor: B")).toBeInTheDocument();
    expect(screen.getByText("Messages: 1")).toBeInTheDocument();
    expect(screen.getByText("Player B: connected")).toBeInTheDocument();
    expect(audio.updateVoiceStatus).toHaveBeenCalledWith(expect.objectContaining({ transcriptionReady: true }));

    act(() => socket.message({ protocol_version: 2, type: "action_rejected", public_session_id: "ROOM1", code: "invalid_action" }));
    expect(await screen.findByText("Action rejected: invalid_action")).toBeInTheDocument();
    act(() => socket.message({ protocol_version: 2, type: "error", public_session_id: "ROOM1", code: "message_too_large", fatal: false }));
    expect(await screen.findByText("The message was too long.")).toBeInTheDocument();
  });

  it("requires consent and exposes the configured participant-information and decline links", async () => {
    const client = api();
    client.getExperiment.mockResolvedValue({
      ...config(),
      participantInformationUrl: "https://study.test/information",
      participantInformationVersion: "v2",
      consents: [{ id: "research", title: "Research use", body: "Use my responses.", required: true }],
      recruitment: { provider: "prolific", decline_url: "https://app.prolific.com/submissions/complete?cc=DECLINE" }
    });
    render(<ParticipantAppTestHarness apiClient={client as never} createAudioController={() => audio as never} renderGame={game} />);

    const enter = await screen.findByRole("button", { name: "Enter waiting room" });
    expect(enter).toBeDisabled();
    expect(screen.getByRole("link", { name: /participant information/i })).toHaveAttribute("href", "https://study.test/information");
    expect(screen.getByRole("link", { name: "Do not consent" })).toHaveAttribute("href", expect.stringContaining("DECLINE"));
    fireEvent.click(screen.getByRole("checkbox", { name: /research use/i }));
    expect(enter).toBeEnabled();
    fireEvent.click(enter);
    await waitFor(() => expect(client.acceptConsents).toHaveBeenCalledWith({ research: true }));
  });

  it("does not fabricate a local refusal for an invalid Prolific configuration", async () => {
    const client = api();
    client.getExperiment.mockResolvedValue({
      ...config(),
      consents: [{ id: "research", title: "Research use", body: "Use my responses.", required: true }],
      recruitment: { provider: "prolific", decline_url: null }
    });
    render(<ParticipantAppTestHarness apiClient={client as never} createAudioController={() => audio as never} renderGame={game} />);

    await screen.findByRole("button", { name: "Enter waiting room" });
    expect(screen.queryByRole("button", { name: "Do not consent" })).not.toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Do not consent" })).not.toBeInTheDocument();
    expect(client.register).not.toHaveBeenCalled();
  });

  it("shows the admitted waiting room while its game channel is retried", async () => {
    const client = api();
    let credentialAvailable = false;
    client.hasCredential = vi.fn(() => credentialAvailable);
    client.register.mockImplementation(async () => { credentialAvailable = true; });
    client.getGameSession
      .mockRejectedValueOnce(new TypeError("temporary channel failure"))
      .mockResolvedValueOnce({ websocketUrl: "/ws/game/ROOM1", token: "ticket" });
    render(<ParticipantAppTestHarness apiClient={client as never} createAudioController={() => audio as never} renderGame={game} />);

    const enter = await screen.findByRole("button", { name: "Enter waiting room" });
    fireEvent.click(enter);
    expect(await screen.findByRole("heading", { name: "Waiting for another participant" })).toBeInTheDocument();
    expect(screen.getByText("temporary channel failure")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Enter waiting room" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Retry connection" }));

    await waitFor(() => expect(FakeWebSocket.instances).toHaveLength(1));
    expect(client.register).toHaveBeenCalledOnce();
    expect(client.join).toHaveBeenCalledOnce();
    expect(client.getGameSession).toHaveBeenCalledTimes(2);
  });

  it.each<[ParticipantOutcome, RegExp]>([
    ["completed", /responses have been recorded/i],
    ["left_waiting_room", /before a playable game began/i],
    ["connection_lost", /connection did not return/i],
    ["partner_left", /partner left or could not reconnect/i],
    ["partner_unavailable", /no partner became available/i],
    ["idle_limit_reached", /neither participant produced meaningful activity/i],
    ["technical_failure", /technical problem prevented/i],
    ["lifetime_limit_reached", /absolute lifetime limit/i]
  ])("renders the %s terminal outcome", async (outcome, expectedText) => {
    const client = api();
    render(<ParticipantAppTestHarness apiClient={client as never} createAudioController={() => audio as never} renderGame={game} />);
    fireEvent.click(await screen.findByRole("button", { name: "Enter waiting room" }));
    await waitFor(() => expect(FakeWebSocket.instances).toHaveLength(1));
    const socket = FakeWebSocket.instances[0];
    act(() => {
      socket.open();
      socket.message({
        protocol_version: 2,
        type: "participant_state",
        participant_state: {
          state: "ended",
          public_session_id: "ROOM1",
          role: "A",
          result: { outcome, reason: "test_reason", completion: null, final_observation: { view: "final" }, handoff: null }
        }
      });
    });
    expect(await screen.findByText(expectedText)).toBeInTheDocument();
  });

  it("renders game completion and copies the Prolific handoff code", async () => {
    const client = api();
    const writeText = vi.fn(async () => undefined);
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
    render(<ParticipantAppTestHarness
      apiClient={client as never}
      createAudioController={() => audio as never}
      renderCompletion={(completion) => <h1>Score: {completion.score}</h1>}
      renderGame={game}
    />);
    fireEvent.click(await screen.findByRole("button", { name: "Enter waiting room" }));
    await waitFor(() => expect(FakeWebSocket.instances).toHaveLength(1));
    const socket = FakeWebSocket.instances[0];
    act(() => {
      socket.open();
      socket.message({
        protocol_version: 2,
        type: "participant_state",
        participant_state: {
          state: "ended",
          public_session_id: "ROOM1",
          role: "A",
          result: {
            outcome: "completed",
            reason: "completed",
            completion: { score: 7 },
            final_observation: { view: "final" },
            handoff: { provider: "prolific", code: "GREENOWL", url: "https://app.prolific.com/submissions/complete?cc=GREENOWL" }
          }
        }
      });
    });

    expect(await screen.findByRole("heading", { name: "Score: 7" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Copy Prolific completion code" }));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith("GREENOWL"));
    expect(await screen.findByText("Copied to clipboard.")).toBeInTheDocument();
  });
});
