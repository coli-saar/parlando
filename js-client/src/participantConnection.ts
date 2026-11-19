import {
  decodeServerMessage, playerMessage, reduceParticipantState, type ParticipantClient,
  type ParticipantState, type PlayerMessage, type PlayerRole, type ServerMessage
} from "./protocol.js";

/** Session projection consumed by the participant view. */
export interface LiveSession<TObservation, TAction, TCompletion> {
  participantState: ParticipantState<TObservation, TAction, TCompletion>;
  transition: { actor: PlayerRole; action: TAction } | null;
  synchronization: "connecting" | "connected" | "reconnecting" | "disconnected" | "leaving";
  conversation: PlayerMessage[];
}

/** The game heartbeat proves transport liveness without extending the activity deadline. */
const CLIENT_HEARTBEAT_INTERVAL_MS = 1_000;

/** View and audio notifications supplied by the participant shell. */
interface ConnectionEvents<TObservation, TAction, TCompletion> {
  /** Publishes the latest renderable state. */
  change(session: LiveSession<TObservation, TAction, TCompletion>): void;
  /** Updates the visible transport error. */
  error(message: string): void;
  /** Applies the terminal audio policy for this response. */
  terminal(snapshot: Extract<ParticipantState<TObservation, TAction, TCompletion>, {state: "ended"}>): void;
  /** Forwards provider readiness to the independent audio controller. */
  voice(status: Extract<ServerMessage, {type: "voice_status"}>["voice"]): void;
}

/** Owns the participant game socket and invalidates asynchronous work when its attempt ends. */
export class ParticipantConnection<TObservation, TAction, TCompletion> {
  private current: LiveSession<TObservation, TAction, TCompletion> | null = null;
  private socket: WebSocket | null = null;
  private generation = 0;
  private retryTimer: number | undefined;
  private heartbeat: number | undefined;
  private retryStarted = 0;
  private retries = 0;

  /** Receives an authenticated API and view notifications for this participant. */
  constructor(private api: ParticipantClient, private events: ConnectionEvents<TObservation, TAction, TCompletion>) {}

  /** Joins once after optional consent setup, ignoring a reply superseded by teardown. */
  async join(prepare?: () => Promise<void>): Promise<void> {
    const attempt = ++this.generation;
    await prepare?.();
    if (attempt !== this.generation) return;
    const snapshot = await this.api.join<TObservation, TAction, TCompletion>();
    if (attempt === this.generation) await this.connect(snapshot);
  }

  /** Retries the admitted participant's game transport without repeating admission. */
  async retry(): Promise<void> {
    if (this.current?.synchronization === "disconnected" && this.current.participantState.state !== "ended") {
      await this.connect(this.current.participantState);
    }
  }

  /** Sends game input only while the current socket owns an active view. */
  act(action: TAction): void {
    if (this.acceptsInput()) this.api.sendAction(this.socket, action);
  }

  /** Sends participant chat under the same lifecycle rules as actions. */
  message(text: string): void {
    if (this.acceptsInput()) this.api.sendMessage(this.socket, text);
  }

  /** Checks current state at send time, including input from a retained game callback. */
  private acceptsInput(): boolean {
    return this.socket !== null && this.current?.synchronization === "connected"
      && this.current.participantState.state === "active";
  }

  /** Waits for a committed leave response before closing the socket. */
  async leave(): Promise<void> {
    const snapshot = this.current?.participantState;
    if (!snapshot || snapshot.state === "registered" || snapshot.state === "ended" || this.current?.synchronization === "leaving") return;
    this.update({ synchronization: "leaving" });
    const attempt = ++this.generation;
    this.clearTimers();
    try {
      const terminal = await this.api.leaveSession<TObservation, TAction, TCompletion>(snapshot.public_session_id);
      if (attempt !== this.generation) return;
      if (terminal.state !== "ended") throw new Error("Expected a terminal participant state after leave.");
      this.applySnapshot(terminal, "disconnected");
    } catch (error) {
      if (attempt === this.generation) {
        this.stop();
        this.update({ synchronization: "reconnecting" });
        this.scheduleRetry();
      }
      throw error;
    }
  }

  /** Invalidates pending requests, timers, and socket callbacks on teardown. */
  stop(): void {
    this.generation++;
    this.clearTimers();
    const socket = this.socket;
    this.socket = null;
    if (socket && socket.readyState < 2) socket.close();
  }

  /** Clears all transport timers before replacing or closing a socket. */
  private clearTimers(): void {
    window.clearTimeout(this.retryTimer);
    window.clearInterval(this.heartbeat);
    this.retryTimer = undefined;
    this.heartbeat = undefined;
  }

  /** Publishes a change synchronously so subsequent events see the same state as the view. */
  private update(patch: Partial<LiveSession<TObservation, TAction, TCompletion>>): void {
    if (!this.current) return;
    this.current = { ...this.current, ...patch };
    this.events.change(this.current);
  }

  /** Reconciles full snapshots; only consecutive live messages must follow adjacent lifecycle edges. */
  private applySnapshot(snapshot: ParticipantState<TObservation, TAction, TCompletion>, synchronization: LiveSession<TObservation, TAction, TCompletion>["synchronization"], consecutive = false): void {
    if (snapshot.state === "registered") return;
    const previous = this.current;
    if (consecutive && previous) reduceParticipantState(previous.participantState, snapshot);
    const sameRoom = previous && previous.participantState.state !== "registered"
      && previous.participantState.public_session_id === snapshot.public_session_id;

    this.current = {
      participantState: snapshot, transition: null, synchronization,
      conversation: sameRoom ? previous.conversation : []
    };
    if (snapshot.state === "ended") {
      this.stop();
      this.events.terminal(snapshot);
    }
    this.events.change(this.current);
  }

  /** Obtains a single-use ticket and installs callbacks bound to this connection attempt. */
  private async connect(snapshot: ParticipantState<TObservation, TAction, TCompletion>): Promise<void> {
    if (snapshot.state === "registered") return;
    this.stop();
    const attempt = this.generation;
    this.applySnapshot(snapshot, "connecting");
    if (snapshot.state === "ended") return;
    let socket: WebSocket;
    try {
      const plan = await this.api.getGameSession(snapshot.public_session_id);
      if (attempt !== this.generation) return;
      socket = new WebSocket(this.api.socketUrl(plan));
      this.socket = socket;
    } catch (error) {
      if (attempt !== this.generation) return;
      this.update({ synchronization: "disconnected" });
      throw error;
    }
    let receivedSnapshot = false;
    socket.addEventListener("open", () => {
      if (attempt !== this.generation) return;
      this.retryStarted = 0;
      this.retries = 0;
      this.events.error("");
      socket.send(JSON.stringify({ type: "ready" }));
      this.heartbeat = window.setInterval(() => {
        if (socket.readyState === WebSocket.OPEN) socket.send(JSON.stringify({ type: "heartbeat" }));
      }, CLIENT_HEARTBEAT_INTERVAL_MS);
      this.update({ synchronization: "connected" });
    });
    socket.addEventListener("error", () => {
      if (attempt === this.generation) this.events.error("Could not connect to the game channel. Retrying…");
    });
    socket.addEventListener("message", (event) => {
      if (attempt !== this.generation) return;
      try {
        const message = decodeServerMessage<TObservation, TAction, TCompletion>(JSON.parse(event.data));
        if (message.type === "participant_state") {
          this.applySnapshot(message.participant_state, "connected", receivedSnapshot);
          receivedSnapshot = true;
        } else {
          this.receive(message);
        }
      } catch {
        this.events.error("The server sent an invalid game message.");
        socket.close(1002, "Invalid server message");
      }
    });
    socket.addEventListener("close", () => {
      if (attempt !== this.generation) return;
      this.clearTimers();
      this.socket = null;
      this.update({ synchronization: "reconnecting" });
      void this.reconcile(++this.generation);
    });
  }

  /** Applies platform messages to the latest projection while keeping game payloads opaque. */
  private receive(message: Exclude<ServerMessage<TObservation, TAction, TCompletion>, {type: "participant_state"}>): void {
    const current = this.current;
    if (!current) return;
    const state = current.participantState;
    switch (message.type) {
      case "transition":
        if (state.state === "active") this.update({
          transition: { actor: message.actor, action: message.action },
          participantState: { ...state, observation: message.observation, available_actions: message.available_actions }
        });
        break;
      case "presence":
        if (state.state === "waiting" || state.state === "active" || state.state === "paused") {
          this.update({ participantState: { ...state, presence: message.presence } });
        }
        break;
      case "message":
        if (!current.conversation.some((entry) => entry.id === message.message.id)) {
          this.update({ conversation: [...current.conversation, playerMessage(message.message)].slice(-50) });
        }
        break;
      case "voice_status": this.events.voice(message.voice); break;
      case "action_rejected": this.events.error(`Action rejected: ${message.code}`); break;
      case "error": this.events.error(errorText(message.code)); break;
    }
  }

  /** Fetches a full snapshot after transport loss, tolerating missed lifecycle messages. */
  private async reconcile(attempt: number): Promise<void> {
    try {
      const snapshot = await this.api.getParticipantState<TObservation, TAction, TCompletion>();
      if (attempt !== this.generation) return;
      this.applySnapshot(snapshot, "reconnecting");
    } catch {
      // The HTTP connection may be unavailable along with the WebSocket.
    }
    if (attempt === this.generation) this.scheduleRetry();
  }

  /** Retries with bounded backoff while the current participant is still in a room. */
  private scheduleRetry(): void {
    const snapshot = this.current?.participantState;
    if (!snapshot || snapshot.state === "ended" || this.retryTimer !== undefined) return;
    this.retryStarted ||= Date.now();
    const remaining = 15_000 - (Date.now() - this.retryStarted);
    if (remaining <= 0) {
      this.update({ synchronization: "disconnected" });
      this.events.error("The connection could not be restored in time.");
      return;
    }
    const delays = [1_000, 2_000, 5_000, 10_000];
    const delay = Math.min(delays[Math.min(this.retries++, delays.length - 1)], remaining);
    this.retryTimer = window.setTimeout(() => {
      this.retryTimer = undefined;
      const request = this.connect(snapshot);
      const attempt = this.generation;
      void request.catch((error) => {
        if (attempt !== this.generation) return;
        this.events.error(error instanceof Error ? error.message : "Could not reconnect to the game channel.");
        this.scheduleRetry();
      });
    }, delay);
  }
}

/** Maps stable protocol error codes onto the standard participant application's copy. */
function errorText(code: string): string {
  const messages: Record<string, string> = {
    action_too_large: "The action was too large.",
    internal_error: "The server could not complete the request.",
    invalid_action: "The action was not valid.",
    invalid_message: "The server could not read the game message.",
    message_rate_limited: "Please wait a moment before sending another message.",
    message_rejected: "The message could not be sent.",
    message_too_large: "The message was too long.",
    participant_left: "This session ended because a player left.",
    readiness_failed: "The session could not be started.",
    session_end_failed: "The session could not be ended cleanly.",
  };
  return messages[code] ?? "The server rejected the last request.";
}
