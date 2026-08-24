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
  /** Human-readable name of the compiled game used for participant-facing identity. */
  gameName: string;
  /** Lifecycle state of the experiment selected by this client's route. */
  status: "inactive" | "testing" | "active" | "completed" | "archived";
  /** Institution operating the experiment, when the server exposes one. */
  institution?: string | null;
  participantInformationVersion?: string | null;
  participantInformationUrl?: string | null;
  consents: ConsentItem[];
  voice?: { enabled?: boolean };
  recruitment?: RecruitmentInfo;
}

interface ExperimentResponse {
  game_name: string;
  experiment_status: ExperimentInfo["status"];
  institution?: string | null;
  participant_information_version?: string | null;
  participant_information_url?: string | null;
  consents: ConsentItem[];
  voice?: { enabled?: boolean };
  recruitment?: RecruitmentInfo;
}

/** Participant-visible recruitment behavior required during intake. */
export interface RecruitmentInfo {
  provider?: "direct" | "prolific";
  decline_url?: string | null;
  return_url?: string | null;
}

export interface ConsentItem {
  id: string;
  title: string;
  /** Plain-text consent copy. */
  body: string;
  required: boolean;
}

/** Why an active participant is temporarily unable to interact. */
export type ParticipantPauseReason = {
  type: "partner_reconnecting";
  deadline_at: string;
};

/** Immutable recipient-specific consequence of one ended session. */
export interface ParticipantResult<TObservation = unknown, TCompletion = Record<string, unknown>> {
  outcome: ParticipantOutcome;
  reason: string;
  completion: TCompletion | null;
  final_observation: TObservation | null;
  handoff: RecruitmentHandoff | null;
}

/** Canonical participant lifecycle shared with the server. */
export type ParticipantState<TObservation = unknown, TAction = unknown, TCompletion = Record<string, unknown>> =
  | { state: "registered" }
  | { state: "waiting"; public_session_id: string; role: PlayerRole; presence: Record<string, unknown> }
  | { state: "active"; public_session_id: string; role: PlayerRole; observation: TObservation; available_actions: TAction[] | null; presence: Record<string, unknown> }
  | { state: "paused"; public_session_id: string; role: PlayerRole; reason: ParticipantPauseReason; observation: TObservation; available_actions: TAction[] | null; presence: Record<string, unknown> }
  | { state: "ended"; public_session_id: string; role: PlayerRole; result: ParticipantResult<TObservation, TCompletion> };

/** Applies a complete snapshot while rejecting impossible lifecycle jumps. */
export function reduceParticipantState<TObservation, TAction, TCompletion>(
  current: ParticipantState<TObservation, TAction, TCompletion> | null,
  next: ParticipantState<TObservation, TAction, TCompletion>
): ParticipantState<TObservation, TAction, TCompletion> {
  if (!current || current.state === next.state) return next;
  const transition = `${current.state}->${next.state}`;
  const allowed = new Set([
    "registered->waiting", "waiting->active", "waiting->ended",
    "active->paused", "active->ended", "paused->active", "paused->ended"
  ]);
  if (!allowed.has(transition)) throw new Error(`invalid participant transition ${transition}`);
  return next;
}

interface ParticipantStateResponse<TObservation = unknown, TAction = unknown, TCompletion = Record<string, unknown>> {
  participant_state: ParticipantState<TObservation, TAction, TCompletion>;
}

export interface ParticipantClientOptions {
  /** Experiment-scoped API root; defaults to the current browser route. */
  baseUrl?: string;
}

/** Provider-neutral consequence shown to one participant after a terminal session event. */
export type ParticipantOutcome =
  | "completed"
  | "withdrew"
  | "partner_left"
  | "partner_unavailable"
  | "timed_out"
  | "technical_failure";

/** Optional external recruitment handoff selected by the server. */
export interface RecruitmentHandoff {
  provider: "prolific";
  code: string;
  url: string;
}

/** Authenticated audio-channel parameters returned for one joined session. */
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

/** Authenticated one-use game-channel parameters returned for one joined session. */
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
> = { protocol_version: 2 } & (
  | {
      type: "participant_state";
      participant_state: ParticipantState<TObservation, TAction, TCompletion>;
    }
  | {
      type: "transition";
      public_session_id: string;
      actor: PlayerRole;
      action: TAction;
      observation: TObservation;
      available_actions: TAction[] | null;
    }
  | { type: "message"; public_session_id: string; message: WirePlayerMessage }
  | { type: "presence"; public_session_id: string; presence: Record<string, unknown> }
  | {
      type: "voice_status";
      public_session_id: string;
      voice: {
        audioReady?: boolean;
        transcriptionReady?: boolean;
        transcriptionStatus?: string;
      };
    }
  | { type: "action_rejected"; public_session_id: string; code: string }
  | { type: "error"; public_session_id: string; code: string; fatal: boolean }
);

/** @internal Decodes and validates one untrusted version-two game-channel message. */
export function decodeServerMessage<
  TObservation = unknown,
  TAction = unknown,
  TCompletion = Record<string, unknown>
>(input: unknown): ServerMessage<TObservation, TAction, TCompletion> {
  if (!isRecord(input) || input.protocol_version !== 2 || typeof input.type !== "string") {
    throw new Error("invalid participant protocol envelope");
  }
  switch (input.type) {
    case "participant_state":
      requireParticipantState(input.participant_state);
      break;
    case "transition":
      requireRole(input, "actor");
      requireField(input, "action");
      requireField(input, "observation");
      requireActions(input);
      break;
    case "message": {
      if (!isRecord(input.message)) throw new Error("invalid player message");
      requireString(input.message, "id");
      requireRole(input.message, "sender");
      requireString(input.message, "text");
      requireString(input.message, "created_at");
      if (input.message.input !== "text" && input.message.input !== "voice_transcript") {
        throw new Error("invalid player message input");
      }
      break;
    }
    case "presence":
      if (!isRecord(input.presence)) throw new Error("invalid presence payload");
      break;
    case "voice_status":
      if (!isRecord(input.voice)) throw new Error("invalid voice-status payload");
      break;
    case "action_rejected":
      requireString(input, "code");
      break;
    case "error":
      requireString(input, "code");
      if (typeof input.fatal !== "boolean") throw new Error("invalid fatal flag");
      break;
    default:
      throw new Error(`unknown participant protocol message ${input.type}`);
  }
  return input as ServerMessage<TObservation, TAction, TCompletion>;
}

/** Validates one complete participant-state snapshot. */
function requireParticipantState(value: unknown): void {
  if (!isRecord(value) || !["registered", "waiting", "active", "paused", "ended"].includes(String(value.state))) {
    throw new Error("invalid participant state");
  }
  if (value.state === "registered") return;
  requireString(value, "public_session_id");
  requireRole(value, "role");
  if (value.state === "waiting") {
    if (!isRecord(value.presence)) throw new Error("invalid participant presence");
    return;
  }
  if (value.state === "ended") {
    if (!isRecord(value.result)) throw new Error("invalid participant result");
    requireOutcome(value.result, "outcome");
    requireString(value.result, "reason");
    return;
  }
  requireField(value, "observation");
  requireActions(value);
  if (!isRecord(value.presence)) throw new Error("invalid participant presence");
  if (value.state === "paused" && !isRecord(value.reason)) throw new Error("invalid pause reason");
}

/** Returns whether one untrusted value is a plain JSON object shape. */
function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Requires one own field even when the game-specific value itself is null. */
function requireField(value: Record<string, unknown>, key: string): void {
  if (!Object.prototype.hasOwnProperty.call(value, key)) throw new Error(`missing ${key}`);
}

/** Requires one non-empty string field. */
function requireString(value: Record<string, unknown>, key: string): void {
  if (typeof value[key] !== "string" || value[key].length === 0) {
    throw new Error(`invalid ${key}`);
  }
}

/** Requires one exact two-player role. */
function requireRole(value: Record<string, unknown>, key: string): void {
  if (value[key] !== "A" && value[key] !== "B") throw new Error(`invalid ${key}`);
}

/** Requires one participant outcome from the stable provider-neutral vocabulary. */
function requireOutcome(value: Record<string, unknown>, key: string): void {
  if (!["completed", "withdrew", "partner_left", "partner_unavailable", "timed_out", "technical_failure"].includes(String(value[key]))) {
    throw new Error(`invalid ${key}`);
  }
}

/** Validates a server-selected recruitment handoff without following it. */
function isRecruitmentHandoff(value: unknown): value is RecruitmentHandoff {
  return isRecord(value) && value.provider === "prolific" && typeof value.code === "string" && typeof value.url === "string";
}

/** Preserves the distinction between an unknown and an empty action catalogue. */
function requireActions(value: Record<string, unknown>): void {
  if (value.available_actions !== null && !Array.isArray(value.available_actions)) {
    throw new Error("invalid available_actions");
  }
}

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
    this.participantCredential = readSessionCredential(this.baseUrl);
  }

  /** Returns whether this tab retains a credential suitable for reload recovery. */
  hasCredential(): boolean {
    return this.participantCredential !== null;
  }

  /** Reads the participant-visible experiment configuration. */
  async getExperiment(): Promise<ExperimentInfo> {
    const experiment = await this.get<ExperimentResponse>("/api/config");
    return {
      gameName: experiment.game_name,
      status: experiment.experiment_status,
      institution: experiment.institution,
      participantInformationVersion: experiment.participant_information_version,
      participantInformationUrl: experiment.participant_information_url,
      consents: experiment.consents,
      voice: experiment.voice,
      recruitment: experiment.recruitment
    };
  }

  /** Registers a participant and retains the returned credential inside this client. */
  async register(): Promise<void> {
    const generation = ++this.participantGeneration;
    const participant = await this.post<ParticipantCreateResponse>(
      "/api/participants",
      prolificIntakeParameters()
    );
    if (generation === this.participantGeneration) {
      this.participantCredential = participant.participant_credential;
      writeSessionCredential(this.baseUrl, participant.participant_credential);
    }
  }

  /** Records this participant's decisions for the experiment's consent items. */
  acceptConsents(decisions: Record<string, boolean>): Promise<void> {
    return this.postAuthenticated("/api/consent", { decisions });
  }

  /** Joins or reuses one session and returns the authoritative participant snapshot. */
  async join<TObservation = unknown, TAction = unknown, TCompletion = Record<string, unknown>>(): Promise<ParticipantState<TObservation, TAction, TCompletion>> {
    const response = await this.postAuthenticated<ParticipantStateResponse<TObservation, TAction, TCompletion>>("/api/sessions", {});
    return response.participant_state;
  }

  /** Reconciles participant lifecycle independently of any WebSocket connection. */
  async getParticipantState<TObservation = unknown, TAction = unknown, TCompletion = Record<string, unknown>>(): Promise<ParticipantState<TObservation, TAction, TCompletion>> {
    const response = await this.getAuthenticated<ParticipantStateResponse<TObservation, TAction, TCompletion>>("/api/participant-state");
    return response.participant_state;
  }

  /** Idempotently leaves a waiting, active, or paused session over reliable HTTP. */
  async leaveSession<TObservation = unknown, TAction = unknown, TCompletion = Record<string, unknown>>(sessionId: string): Promise<ParticipantState<TObservation, TAction, TCompletion>> {
    const response = await this.postAuthenticated<ParticipantStateResponse<TObservation, TAction, TCompletion>>(`/api/sessions/${sessionId}/leave`, {});
    return response.participant_state;
  }

  /** Obtains an authenticated audio plan when a custom client needs voice transport. */
  async getAudioSession(sessionId: string): Promise<AudioSessionPlan> {
    const plan = await this.postAuthenticated<AudioSessionResponse>(`/api/sessions/${sessionId}/audio-session`, {});
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
  async getGameSession(sessionId: string): Promise<GameSessionPlan> {
    const plan = await this.postAuthenticated<GameSessionResponse>(`/api/sessions/${sessionId}/game-session`, {});
    return { websocketUrl: plan.websocket_url, token: plan.token };
  }

  /** @internal Records transport diagnostics for the standard participant application. */
  postVoiceDiagnostic(
    sessionId: string,
    event: string,
    metadata: Record<string, unknown> = {}
  ): void {
    try {
      void fetch(`${this.baseUrl}/api/sessions/${sessionId}/voice-diagnostics`, {
        method: "POST",
        headers: this.authHeaders(),
        body: JSON.stringify({
          event,
          metadata
        }),
        keepalive: true
      }).catch(() => undefined);
    } catch {
      // Diagnostics are best-effort and must never interrupt participant interaction.
    }
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

  private getAuthenticated<T>(path: string): Promise<T> {
    return checkedJson(fetch(`${this.baseUrl}${path}`, { headers: this.authHeaders() }));
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

/** Reads the current Prolific query parameters and removes them from the visible URL. */
function prolificIntakeParameters(): Record<string, unknown> {
  if (typeof window === "undefined") return {};
  const query = new URLSearchParams(window.location.search);
  const participantId = query.get("PROLIFIC_PID");
  const studyId = query.get("STUDY_ID");
  const sessionId = query.get("SESSION_ID");
  if (!participantId && !studyId && !sessionId) return {};
  if (!participantId || !studyId || !sessionId) throw new Error("The Prolific link is missing required parameters.");
  for (const key of ["PROLIFIC_PID", "STUDY_ID", "SESSION_ID"]) query.delete(key);
  const suffix = query.toString();
  window.history.replaceState(null, "", `${window.location.pathname}${suffix ? `?${suffix}` : ""}${window.location.hash}`);
  return { prolific: { participant_id: participantId, study_id: studyId, session_id: sessionId } };
}

/** Loads one tab-scoped participant credential while tolerating disabled browser storage. */
function readSessionCredential(baseUrl: string): string | null {
  if (typeof window === "undefined") return null;
  try {
    return window.sessionStorage.getItem(`parlando.participant.${baseUrl}`);
  } catch {
    return null;
  }
}

/** Persists one credential only for this browser tab and experiment route. */
function writeSessionCredential(baseUrl: string, credential: string): void {
  if (typeof window === "undefined") return;
  try {
    window.sessionStorage.setItem(`parlando.participant.${baseUrl}`, credential);
  } catch {
    // Reload recovery is best-effort when storage is disabled.
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
