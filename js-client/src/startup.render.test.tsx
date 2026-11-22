// @vitest-environment happy-dom

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { initialVoicePreflight, initialVoiceStatus } from "./audio/types";
import type { ExperimentInfo, ParticipantOutcome, ParticipantState } from "./protocol";
import {
  ParticipantApp,
  ReplayPlayer,
  type GameSnapshot,
  type SessionReplay
} from "./startup";
import { IdleDeadlineNotice, WaitingRoomNotice } from "./participant/deadlines";
import { FarewellVoiceNotice } from "./participant/outcome";
import { VoicePreparationControls } from "./participant/microphone";
import { replayRequestUrl } from "./participant/replay";
import { CommunicationWidget } from "./react";
import { ParticipantAppRuntime } from "./participant/runtime";

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
  return { gameName: "Tiny Game", status: "active", consents: [], communication: { mode: "typed" } };
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

function game(session: GameSnapshot<{ view: string }, { type: string }>) {
  return <div>
    <span>{session.observation.view}</span>
    <span>Source: {session.recruitmentSource ?? "unknown"}</span>
    <span>{String(session.interactionEnabled)}</span>
    <span>Actor: {session.transition?.actor ?? "none"}</span>
  </div>;
}

beforeEach(() => { vi.clearAllMocks(); FakeWebSocket.instances = []; vi.stubGlobal("WebSocket", FakeWebSocket); Object.defineProperty(navigator, "mediaDevices", { configurable: true, value: { enumerateDevices: vi.fn(async () => []), addEventListener: vi.fn(), removeEventListener: vi.fn() } }); });
afterEach(() => { cleanup(); vi.useRealTimers(); vi.unstubAllGlobals(); vi.restoreAllMocks(); });

describe("microphone preparation controls", () => {
  it("prepares the default input and switches only between available named microphones", () => {
    const prepare = vi.fn();
    const inputs = ["default", "built-in", "headset"].map((deviceId) => ({
      kind: "audioinput", deviceId, label: deviceId,
    } as MediaDeviceInfo));
    const props = { audioInputs: inputs, enabled: true, onPrepareVoice: prepare, selectedAudioInputId: "built-in" };
    const { rerender } = render(<VoicePreparationControls {...props} />);
    fireEvent.click(screen.getByRole("button", { name: "Prepare voice" }));
    const ready = { ...initialVoicePreflight, ready: true };
    rerender(<VoicePreparationControls {...props} voicePreflight={ready} />);
    const selector = screen.getByRole("combobox", { name: "Microphone input" });
    expect(screen.getAllByRole("option")).toHaveLength(2);
    expect(selector).toHaveValue("built-in");
    fireEvent.change(selector, { target: { value: "headset" } });
    expect(prepare.mock.calls).toEqual([[""], ["headset"]]);
    rerender(<VoicePreparationControls {...props} voicePreflight={{ ...ready, preparing: true }} />);
    expect(selector).toBeDisabled();
    rerender(<VoicePreparationControls {...props} audioInputs={[inputs[1]]} voicePreflight={ready} />);
    expect(selector).toBeDisabled();
  });
});

describe("administrator replay startup", () => {
  afterEach(() => { window.history.replaceState(null, "", "/"); });

  it.each([
    ["http://[", null],
    ["https://other.example/api/admin/replay", null],
    ["/api/config", null],
    ["/api/admin/replay?session=123#frame", "/api/admin/replay?session=123"],
  ])("validates the dashboard replay address %s", (candidate, expected) => {
    window.history.replaceState(null, "", "?" + new URLSearchParams({ parlandoReplay: candidate! }));
    expect(replayRequestUrl()).toBe(expected);
  });

  it.each([undefined, { mode: "voice" }])("reports invalid replay communication %j before rendering", async (communication) => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValueOnce(Response.json({ communication })));
    window.history.replaceState(null, "", "?parlandoReplay=/api/admin/replay");
    render(<ParticipantApp renderGame={game} />);
    expect(await screen.findByRole("alert")).toHaveTextContent(
      'Replay must specify communication.mode as "typed" or "speech".'
    );
  });

  it("loads only the current replay and clears a previous request's error", async () => {
    const fetchReplay = vi.fn<typeof fetch>();
    vi.stubGlobal("fetch", fetchReplay);
    window.history.replaceState(null, "", "?parlandoReplay=/api/admin/first");
    fetchReplay.mockResolvedValueOnce(new Response("Replay unavailable", { status: 404 }));
    const { rerender } = render(<ParticipantApp renderGame={game} />);
    expect(await screen.findByRole("alert")).toHaveTextContent("Replay unavailable");

    let finishSecond!: (response: Response) => void;
    fetchReplay.mockReturnValueOnce(new Promise((resolve) => { finishSecond = resolve; }));
    window.history.replaceState(null, "", "?parlandoReplay=/api/admin/second");
    rerender(<ParticipantApp renderGame={game} />);
    expect(screen.getByRole("status")).toHaveTextContent("Loading replay");

    window.history.replaceState(null, "", "?parlandoReplay=/api/admin/third");
    fetchReplay.mockResolvedValueOnce(Response.json({ communication: { mode: "typed" }, roles: { A: [], B: [] } }));
    rerender(<ParticipantApp renderGame={game} />);
    expect(await screen.findByRole("alert")).toHaveTextContent("This replay has no frames.");
    await act(async () => { finishSecond(new Response("stale failure", { status: 404 })); });
    expect(screen.getByRole("alert")).toHaveTextContent("This replay has no frames.");
    expect(fetchReplay.mock.calls.map(([url]) => url)).toEqual([
      "/api/admin/first", "/api/admin/second", "/api/admin/third",
    ]);
  });
});

describe("ReplayPlayer", () => {
  it.each([
    ["typed", "default"],
    ["speech", "game"],
  ] as const)("jumps between reconstructed events in %s mode with %s placement", (mode, placement) => {
    vi.useFakeTimers();
    const replay: SessionReplay<{ view: string }, { type: string }, { score: number }> = {
      experiment_id: "demo",
      session_id: 7,
      public_session_id: "ROOM1",
      communication: { mode },
      compatibility: { state: "1", action: "1", rules: "1" },
      roles: {
        A: [
          { event_index: 1, game_time_ms: 0, observation: { view: "A start" }, available_actions: null, transition: null, conversation: [], completion: null },
          { event_index: 4, game_time_ms: 800, observation: { view: "A moved" }, available_actions: null, transition: { actor: "B", action: { type: "move" } }, conversation: [{ id: "m1", sender: "A", text: "go", input: mode === "typed" ? "text" : "voice_transcript", createdAt: "" }], completion: null }
        ],
        B: [
          { event_index: 1, game_time_ms: 0, observation: { view: "B start" }, available_actions: null, transition: null, conversation: [], completion: null },
          { event_index: 4, game_time_ms: 800, observation: { view: "B moved" }, available_actions: null, transition: { actor: "B", action: { type: "move" } }, conversation: [{ id: "m1", sender: "A", text: "go", input: mode === "typed" ? "text" : "voice_transcript", createdAt: "" }], completion: null }
        ]
      }
    };
    render(<ReplayPlayer
      communicationPlacement={placement}
      replay={replay}
      renderGame={(snapshot) => <>{game(snapshot)}{placement === "game" && <CommunicationWidget />}</>}
    />);
    expect(screen.getByText("A start")).toBeInTheDocument();
    expect(screen.getByText("Source: unknown")).toBeInTheDocument();
    expect(screen.getByText("false")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Next event" }));
    expect(screen.getByText("A moved")).toBeInTheDocument();
    expect(screen.getByText("Actor: B")).toBeInTheDocument();
    const widget = screen.getByText("go").closest("section")!;
    expect(widget).toHaveAttribute("data-mode", mode);
    expect(Boolean(widget.closest(".parlando-replay-surface"))).toBe(placement === "game");
    expect(widget.querySelector("input, button")).toBeNull();
    fireEvent.change(screen.getByRole("combobox", { name: "Replay viewpoint" }), { target: { value: "B" } });
    expect(screen.getByText("B moved")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Play replay" }));
    expect(screen.getByText("B start")).toBeInTheDocument();
    act(() => { vi.advanceTimersByTime(799); });
    expect(screen.getByText("B start")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Pause replay" }));
    act(() => { vi.advanceTimersByTime(800); });
    expect(screen.getByText("B start")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Play replay" }));
    act(() => { vi.advanceTimersByTime(800); });
    expect(screen.getByText("B moved")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Next event" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Play replay" })).toBeEnabled();
  });
});

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

    render(<ParticipantAppRuntime apiClient={client as never} createAudioController={() => audio as never} renderGame={game} />);

    expect(await screen.findByText(/responses have been recorded/i)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Enter waiting room" })).not.toBeInTheDocument();
    expect(client.register).not.toHaveBeenCalled();
    expect(client.getGameSession).not.toHaveBeenCalled();
  });

  it("moves Waiting to Active only from an authoritative snapshot", async () => {
    const client = api();
    render(<ParticipantAppRuntime apiClient={client as never} createAudioController={() => audio as never} renderGame={game} />);
    fireEvent.click(await screen.findByRole("button", { name: "Enter waiting room" }));
    await waitFor(() => expect(FakeWebSocket.instances).toHaveLength(1));
    const socket = FakeWebSocket.instances[0];
    act(() => socket.open());
    expect(screen.queryByText("live")).not.toBeInTheDocument();
    act(() => socket.message({ protocol_version: 2, type: "participant_state", participant_state: { ...active, recruitment_source: "prolific" } }));
    expect(screen.getByText("Source: prolific")).toBeInTheDocument();
    expect(await screen.findByText("live")).toBeInTheDocument();
    expect(screen.getByText("true")).toBeInTheDocument();
  });

  it("ignores a pending game ticket after the HTTP leave has committed", async () => {
    const client = api();
    let release!: (plan: { websocketUrl: string; token: string }) => void;
    client.getGameSession.mockImplementation(() => new Promise((resolve) => { release = resolve; }));
    render(<ParticipantAppRuntime apiClient={client as never} createAudioController={() => audio as never} renderGame={game} />);
    fireEvent.click(await screen.findByRole("button", { name: "Enter waiting room" }));
    fireEvent.click(await screen.findByRole("button", { name: "Leave waiting room" }));
    await waitFor(() => expect(client.leaveSession).toHaveBeenCalledOnce());
    await act(async () => { release({ websocketUrl: "/unused", token: "late" }); });
    expect(FakeWebSocket.instances).toHaveLength(0);
    expect(screen.queryByRole("button", { name: "Leave waiting room" })).not.toBeInTheDocument();
  });

  it.each([false, true])("applies farewell voice policy to HTTP terminal reconciliation (farewell %s)", async (farewell) => {
    const client = api();
    client.getParticipantState.mockResolvedValueOnce({
      state: "ended", public_session_id: "ROOM1", role: "A",
      voice_ends_at: farewell ? "2099-01-01T00:00:00Z" : null,
      result: { outcome: "completed", reason: "game_completed", final_observation: {}, completion: null, handoff: null }
    });
    render(<ParticipantAppRuntime apiClient={client as never} createAudioController={() => audio as never} renderGame={game} />);
    fireEvent.click(await screen.findByRole("button", { name: "Enter waiting room" }));
    await waitFor(() => expect(FakeWebSocket.instances).toHaveLength(1));
    act(() => FakeWebSocket.instances[0].close());
    expect(await screen.findByText(/responses have been recorded/i)).toBeInTheDocument();
    expect(audio.disconnect).toHaveBeenCalledTimes(farewell ? 0 : 1);
    expect(client.getGameSession).toHaveBeenCalledOnce();
  });

  it("checks retained game callbacks against the current participant state", async () => {
    const client = api();
    let send!: () => void;
    render(<ParticipantAppRuntime apiClient={client as never} createAudioController={() => audio as never} renderGame={snapshot => {
      if (snapshot.interactionEnabled) send = () => snapshot.act({ type: "move" });
      return game(snapshot);
    }} />);
    fireEvent.click(await screen.findByRole("button", { name: "Enter waiting room" }));
    await waitFor(() => expect(FakeWebSocket.instances).toHaveLength(1));
    const socket = FakeWebSocket.instances[0];
    act(() => {
      socket.open();
      socket.message({ protocol_version: 2, type: "participant_state", participant_state: active });
    });
    send();
    act(() => socket.message({ protocol_version: 2, type: "participant_state", participant_state: {
      ...active, state: "paused", reason: { type: "partner_reconnecting", deadline_at: "2099-01-01T00:00:00Z" }
    } }));
    send();
    expect(client.sendAction).toHaveBeenCalledOnce();
    expect(client.sendAction).toHaveBeenCalledWith(socket, { type: "move" });
  });

  it("renders Paused while strictly disabling interaction", async () => {
    const client = api();
    render(<ParticipantAppRuntime apiClient={client as never} createAudioController={() => audio as never} renderGame={game} />);
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
    render(<ParticipantAppRuntime apiClient={client as never} createAudioController={() => audio as never} renderGame={game} />);
    fireEvent.click(await screen.findByRole("button", { name: "Enter waiting room" }));
    await waitFor(() => expect(FakeWebSocket.instances).toHaveLength(1));
    const socket = FakeWebSocket.instances[0]; act(() => { socket.open(); socket.message({ protocol_version: 2, type: "participant_state", participant_state: active }); });
    fireEvent.click(await screen.findByRole("button", { name: "Leave game" }));
    await waitFor(() => expect(client.leaveSession).toHaveBeenCalledWith("ROOM1"));
    await waitFor(() => expect(socket.close).toHaveBeenCalled());
    expect(await screen.findByText(/left after the game started/i)).toBeInTheDocument();
  });

  it("applies every live server update to the public game session", async () => {
    const client = api();
    render(<ParticipantAppRuntime apiClient={client as never} createAudioController={() => audio as never} renderGame={game} />);
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
    expect(screen.getByText("hello")).toBeInTheDocument();
    expect(screen.getByText(/Player A · Connected/)).toBeInTheDocument();
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
    render(<ParticipantAppRuntime apiClient={client as never} createAudioController={() => audio as never} renderGame={game} />);

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
    render(<ParticipantAppRuntime apiClient={client as never} createAudioController={() => audio as never} renderGame={game} />);

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
    render(<ParticipantAppRuntime apiClient={client as never} createAudioController={() => audio as never} renderGame={game} />);

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

  it("explains a direct waiting deadline without mentioning Prolific payment", async () => {
    const client = api();
    render(<ParticipantAppRuntime apiClient={client as never} createAudioController={() => audio as never} renderGame={game} />);

    fireEvent.click(await screen.findByRole("button", { name: "Enter waiting room" }));

    expect(await screen.findByText(/session will end without starting the game/i)).toBeInTheDocument();
    expect(screen.queryByText(/Prolific|pay you/i)).not.toBeInTheDocument();
  });

  it("explains the Prolific return and manual waiting payment", async () => {
    const client = api();
    client.getExperiment.mockResolvedValue({
      ...config(),
      recruitment: { provider: "prolific", decline_url: null }
    });
    render(<ParticipantAppRuntime apiClient={client as never} createAudioController={() => audio as never} renderGame={game} />);

    fireEvent.click(await screen.findByRole("button", { name: "Enter waiting room" }));

    expect(await screen.findByText(/asked to return your submission/i)).toBeInTheDocument();
    expect(screen.getByText(/researcher can issue a partial payment for the time you waited/i)).toBeInTheDocument();
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
    render(<ParticipantAppRuntime apiClient={client as never} createAudioController={() => audio as never} renderGame={game} />);
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
    expect(screen.queryByText(/prolific/i)).not.toBeInTheDocument();
  });

  it("renders game completion and copies the Prolific handoff code", async () => {
    const client = api();
    const writeText = vi.fn(async () => undefined);
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
    render(<ParticipantAppRuntime
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
    expect(screen.getByText(/responses have been recorded/i)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Copy Prolific completion code" }));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith("GREENOWL"));
    expect(await screen.findByText("Copied to clipboard.")).toBeInTheDocument();
  });

  it("keeps connected voice open after normal completion until the participant ends it", async () => {
    const client = api();
    const connectedStatus = { ...initialVoiceStatus, connected: true, microphoneEnabled: true, message: "Microphone live" };
    const farewellAudio = {
      ...audio,
      disconnect: vi.fn(async () => undefined),
      snapshot: () => ({ voiceStatus: connectedStatus, voicePreflight: initialVoicePreflight }),
      subscribe: (listener: (value: unknown) => void) => { listener({ voiceStatus: connectedStatus, voicePreflight: initialVoicePreflight }); return () => undefined; }
    };
    render(<ParticipantAppRuntime apiClient={client as never} createAudioController={() => farewellAudio as never} renderGame={game} />);
    fireEvent.click(await screen.findByRole("button", { name: "Enter waiting room" }));
    await waitFor(() => expect(FakeWebSocket.instances).toHaveLength(1));
    const socket = FakeWebSocket.instances[0];
    act(() => {
      socket.open();
      socket.message({ protocol_version: 2, type: "participant_state", participant_state: active });
      socket.message({
        protocol_version: 2,
        type: "participant_state",
        participant_state: {
          state: "ended",
          public_session_id: "ROOM1",
          role: "A",
          voice_ends_at: "2099-01-01T00:01:00Z",
          result: { outcome: "completed", reason: "game_completed", completion: null, final_observation: { view: "final" }, handoff: null }
        }
      });
    });

    expect(await screen.findByText("Voice chat remains open")).toBeInTheDocument();
    expect(screen.getByText(/not transcribed or stored/i)).toBeInTheDocument();
    expect(farewellAudio.disconnect).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "End voice chat" }));
    await waitFor(() => expect(farewellAudio.disconnect).toHaveBeenCalledWith(true));
  });
});

describe("FarewellVoiceNotice", () => {
  it("closes voice when the authoritative deadline arrives", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-09-28T20:00:00Z"));
    const onEnd = vi.fn();
    render(<FarewellVoiceNotice
      deadlineAt="2026-09-28T20:00:02Z"
      onEnd={onEnd}
      onMutedChange={vi.fn()}
      status={{ ...initialVoiceStatus, connected: true, microphoneEnabled: true }}
    />);

    expect(screen.getByText("0:02 remaining")).toBeInTheDocument();
    act(() => vi.advanceTimersByTime(2_000));
    expect(onEnd).toHaveBeenCalledOnce();
    expect(screen.getByText("Voice chat has closed.")).toBeInTheDocument();
    act(() => vi.advanceTimersByTime(5_000));
    expect(onEnd).toHaveBeenCalledOnce();
  });
});

describe("deadline notices", () => {
  it("formats and advances the waiting deadline from the authoritative timestamps", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-09-29T12:00:00Z"));
    render(<WaitingRoomNotice
      deadlineAt="2026-09-29T12:01:00Z"
      prolific={false}
      startedAt="2026-09-29T12:00:00Z"
    />);

    expect(screen.getByRole("status")).toHaveTextContent("1:00 remaining");
    act(() => vi.advanceTimersByTime(1_250));
    expect(screen.getByRole("status")).toHaveTextContent("0:59 remaining");
    act(() => vi.advanceTimersByTime(60_000));
    expect(screen.getByRole("status")).toHaveTextContent("0:00 remaining");
    expect(screen.getByRole("status")).not.toHaveTextContent("-1");
  });

  it("keeps direct and Prolific waiting guidance distinct", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-09-29T12:00:00Z"));
    const { rerender } = render(<WaitingRoomNotice
      deadlineAt="2026-09-29T12:02:00Z"
      prolific={false}
      startedAt="2026-09-29T12:00:00Z"
    />);
    expect(screen.getByRole("status")).toHaveTextContent("maximum wait is 2 minutes");
    expect(screen.getByRole("status")).not.toHaveTextContent("Prolific");

    rerender(<WaitingRoomNotice
      deadlineAt="2026-09-29T12:02:00Z"
      prolific
      startedAt="2026-09-29T12:00:00Z"
    />);
    expect(screen.getByRole("status")).toHaveTextContent("return to Prolific");
    expect(screen.getByRole("status")).toHaveTextContent("partial payment");
  });

  it("shows the idle warning only during its final minute and clamps at zero", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-09-29T12:00:00Z"));
    const { rerender } = render(<IdleDeadlineNotice deadlineAt="2026-09-29T12:01:01Z" />);
    expect(screen.queryByRole("status")).not.toBeInTheDocument();

    rerender(<IdleDeadlineNotice deadlineAt="2026-09-29T12:01:00Z" />);
    expect(screen.getByRole("status")).toHaveTextContent("60 seconds");
    act(() => vi.advanceTimersByTime(59_000));
    expect(screen.getByRole("status")).toHaveTextContent("1 second");
    act(() => vi.advanceTimersByTime(2_000));
    expect(screen.getByRole("status")).toHaveTextContent("0 seconds");
  });

  it("clears a waiting countdown interval when it unmounts", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-09-29T12:00:00Z"));
    const clearInterval = vi.spyOn(window, "clearInterval");
    const view = render(<WaitingRoomNotice
      deadlineAt="2026-09-29T12:01:00Z"
      prolific={false}
      startedAt="2026-09-29T12:00:00Z"
    />);
    view.unmount();
    expect(clearInterval).toHaveBeenCalledOnce();
  });
});
