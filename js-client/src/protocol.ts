interface ParticipantCreateResponse {
  participant_credential: string;
  participant_id: string;
}

export type PlayerRole = "A" | "B";

export type MessageInput = "text" | "voice_transcript";

export interface PlayerMessage {
  id: string;
  sender: PlayerRole;
  text: string;
  input: MessageInput;
  createdAt: string;
}

export interface PlayerPresence {
  connected?: boolean;
  audioReady?: boolean;
}

export type Presence = Partial<Record<PlayerRole, PlayerPresence>>;

export interface ExperimentInfo {
  /** Lifecycle state of the experiment selected by this client's route. */
  status: "inactive" | "testing" | "active" | "completed" | "archived";
  /** Institution operating the experiment, when the server exposes one. */
  institution?: string | null;
  participantInformationVersion?: string | null;
  participantInformationUrl?: string | null;
  consents: ConsentItem[];
  voice?: { enabled?: boolean };
}

interface ExperimentResponse {
  experiment_status: ExperimentInfo["status"];
  institution?: string | null;
  participant_information_version?: string | null;
  participant_information_url?: string | null;
  consents: ConsentItem[];
  voice?: { enabled?: boolean };
}

export interface ConsentItem {
  id: string;
  title: string;
  /** Plain-text consent copy. */
  body: string;
  required: boolean;
}

interface RoomResponse<TObservation = unknown, TAction = unknown> {
  room_id: string;
  role: PlayerRole;
  presence?: Record<string, unknown>;
  observation?: TObservation | null;
  available_actions: TAction[] | null;
}

export interface JoinedRoom<TObservation = unknown, TAction = unknown> {
  roomId: string;
  role: PlayerRole;
  presence: Presence;
  observation: TObservation | null;
  availableActions: TAction[] | null;
}

export interface ParticipantClientOptions {
  /** Experiment-scoped API root; defaults to the current browser route. */
  baseUrl?: string;
}

/** Authenticated audio-channel parameters returned for one joined room. */
export interface AudioSessionPlan {
  enabled: boolean;
  websocketUrl: string | null;
  token: string | null;
  protocolVersion: number;
  sampleRateHz: number;
  channels: number;
  frameDurationMs: number;
  jitterBufferMs: number;
}

/** Authenticated one-use game-channel parameters returned for one joined room. */
export interface GameSessionPlan {
  websocketUrl: string;
  token: string;
}

interface AudioSessionResponse {
  enabled: boolean;
  websocket_url?: string | null;
  token?: string | null;
  protocol_version: number;
  sample_rate_hz: number;
  channels: number;
  frame_duration_ms: number;
  jitter_buffer_ms: number;
}

interface GameSessionResponse {
  websocket_url: string;
  token: string;
}

interface WirePlayerMessage {
  id: string;
  sender: PlayerRole;
  text: string;
  input: MessageInput;
  created_at: string;
}

/** @internal Wire messages consumed by the standard participant application. */
export type ServerMessage<
  TObservation = unknown,
  TAction = unknown,
  TCompletion = Record<string, unknown>
> = { protocol_version: 1 } & (
  | {
      type: "session_started";
      room_id: string;
      role: PlayerRole;
      observation: TObservation;
      available_actions: TAction[] | null;
    }
  | {
      type: "transition";
      room_id: string;
      actor: PlayerRole;
      action: TAction;
      observation: TObservation;
      available_actions: TAction[] | null;
    }
  | { type: "message"; room_id: string; message: WirePlayerMessage }
  | { type: "completed"; room_id: string; completion: TCompletion }
  | { type: "abandoned"; room_id: string; code: string }
  | { type: "presence"; room_id: string; presence: Record<string, unknown> }
  | {
      type: "voice_status";
      room_id: string;
      voice: {
        audioReady?: boolean;
        transcriptionReady?: boolean;
        transcriptionStatus?: string;
      };
    }
  | { type: "action_rejected"; room_id: string; code: string }
  | { type: "error"; room_id: string; code: string; fatal: boolean }
);

/** @internal Resolves the default experiment-scoped API root. */
export function apiBase(): string {
  const experimentPath = window.location.pathname.match(/^\/e\/[^/]+/)?.[0] || "";
  return `${window.location.origin}${experimentPath}`;
}

/** @internal Adds a one-use credential to a game WebSocket URL. */
export function socketUrl(websocketUrl: string, token: string): string {
  const base = new URL(websocketUrl, window.location.origin);
  base.protocol = base.protocol === "https:" ? "wss:" : "ws:";
  base.searchParams.set("token", token);
  return base.toString();
}

export class ParticipantClient {
  private readonly baseUrl: string;
  private participantCredential: string | null = null;
  private participantGeneration = 0;

  /** Creates a managed client for one experiment-scoped API root. */
  constructor(options: ParticipantClientOptions = {}) {
    this.baseUrl = options.baseUrl ?? apiBase();
  }

  /** Reads the participant-visible experiment configuration. */
  async getExperiment(): Promise<ExperimentInfo> {
    const experiment = await this.get<ExperimentResponse>("/api/config");
    return {
      status: experiment.experiment_status,
      institution: experiment.institution,
      participantInformationVersion: experiment.participant_information_version,
      participantInformationUrl: experiment.participant_information_url,
      consents: experiment.consents,
      voice: experiment.voice
    };
  }

  /** Registers a participant and retains the returned credential inside this client. */
  async register(): Promise<void> {
    const generation = ++this.participantGeneration;
    const participant = await this.post<ParticipantCreateResponse>(
      "/api/participants",
      {}
    );
    if (generation === this.participantGeneration) {
      this.participantCredential = participant.participant_credential;
    }
  }

  /** Records this participant's decisions for the experiment's consent items. */
  acceptConsents(decisions: Record<string, boolean>): Promise<void> {
    return this.postAuthenticated("/api/consent", { decisions });
  }

  /** Joins or waits for one room using the retained participant credential. */
  async join<TObservation = unknown, TAction = unknown>(): Promise<JoinedRoom<TObservation, TAction>> {
    const room = await this.postAuthenticated<RoomResponse<TObservation, TAction>>("/api/rooms", {});
    return {
      roomId: room.room_id,
      role: room.role,
      presence: normalizePresence(room.presence),
      observation: room.observation ?? null,
      availableActions: room.available_actions
    };
  }

  /** Obtains an authenticated audio plan when a custom client needs voice transport. */
  async getAudioSession(roomId: string): Promise<AudioSessionPlan> {
    const plan = await this.postAuthenticated<AudioSessionResponse>(`/api/rooms/${roomId}/audio-session`, {});
    return {
      enabled: plan.enabled,
      websocketUrl: plan.websocket_url ?? null,
      token: plan.token ?? null,
      protocolVersion: plan.protocol_version,
      sampleRateHz: plan.sample_rate_hz,
      channels: plan.channels,
      frameDurationMs: plan.frame_duration_ms,
      jitterBufferMs: plan.jitter_buffer_ms
    };
  }

  /** Obtains an authenticated one-use game-channel plan for a custom client. */
  async getGameSession(roomId: string): Promise<GameSessionPlan> {
    const plan = await this.postAuthenticated<GameSessionResponse>(`/api/rooms/${roomId}/game-session`, {});
    return { websocketUrl: plan.websocket_url, token: plan.token };
  }

  /** @internal Records transport diagnostics for the standard participant application. */
  postVoiceDiagnostic(
    roomId: string,
    event: string,
    metadata: Record<string, unknown> = {}
  ): void {
    void fetch(`${this.baseUrl}/api/rooms/${roomId}/voice-diagnostics`, {
      method: "POST",
      headers: this.authHeaders(),
      body: JSON.stringify({
        event,
        metadata
      }),
      keepalive: true
    }).catch(() => undefined);
  }

  /** @internal Sends one action for the standard participant application. */
  sendAction<TAction>(socket: WebSocket | null, action: TAction): void {
    if (socket?.readyState !== WebSocket.OPEN) return;
    try {
      socket.send(JSON.stringify({ type: "action", action }));
    } catch {
      // A close may race the ready-state check; the reconnect owner handles recovery.
    }
  }

  /** @internal Sends one message for the standard participant application. */
  sendMessage(socket: WebSocket | null, text: string): void {
    if (socket?.readyState !== WebSocket.OPEN) return;
    try {
      socket.send(JSON.stringify({ type: "message", text }));
    } catch {
      // A close may race the ready-state check; the reconnect owner handles recovery.
    }
  }

  /** @internal Declares an intentional participant departure before closing the game channel. */
  leaveSession(socket: WebSocket | null): void {
    if (socket?.readyState === WebSocket.OPEN) socket.send(JSON.stringify({ type: "leave" }));
  }

  /** @internal Resolves the standard participant application's game-channel URL. */
  socketUrl(plan: GameSessionPlan): string {
    return socketUrl(plan.websocketUrl, plan.token);
  }

  private get<T>(path: string): Promise<T> {
    return checkedJson(fetch(`${this.baseUrl}${path}`));
  }

  private post<T>(path: string, body: unknown): Promise<T> {
    return checkedJson(
      fetch(`${this.baseUrl}${path}`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body)
      })
    );
  }

  private postAuthenticated<T>(path: string, body: unknown): Promise<T> {
    return checkedJson(
      fetch(`${this.baseUrl}${path}`, {
        method: "POST",
        headers: this.authHeaders(),
        body: JSON.stringify(body)
      })
    );
  }

  private authHeaders(): Record<string, string> {
    const credential = this.participantCredential;
    if (!credential) throw new Error("No participant credential is available for this session.");
    return {
      "Content-Type": "application/json",
      Authorization: `Bearer ${credential}`
    };
  }
}

/** @internal Converts a wire message into the small public player-message value. */
export function playerMessage(message: WirePlayerMessage): PlayerMessage {
  return {
    id: message.id,
    sender: message.sender,
    text: message.text,
    input: message.input,
    createdAt: message.created_at
  };
}

/** Normalizes untrusted presence JSON onto the two public player roles. */
function normalizePresence(presence: Record<string, unknown> | undefined): Presence {
  const normalized: Presence = {};
  for (const role of ["A", "B"] as const) {
    const value = presence?.[role];
    if (typeof value !== "object" || value === null) continue;
    const record = value as Record<string, unknown>;
    normalized[role] = {
      connected: Boolean(record.connected),
      audioReady: Boolean(record.audioReady)
    };
  }
  return normalized;
}

/** @internal Decodes one HTTP response for the browser client. */
export async function checkedJson<T>(responsePromise: Promise<Response> | Response): Promise<T> {
  const response = await responsePromise;
  if (!response.ok) {
    const detail = await response.text();
    throw new Error(detail || `Request failed with ${response.status}`);
  }
  if (response.status === 204 || response.headers.get("Content-Length") === "0") {
    return undefined as T;
  }
  const text = await response.text();
  if (!text) return undefined as T;
  try {
    return JSON.parse(text) as T;
  } catch {
    throw new Error(`Response contained invalid JSON (${response.status})`);
  }
}
