export interface ParticipantCreateResponse {
  participant_session_id: string;
  participant_credential: string;
  source: "direct";
  participant_id: string;
}

export interface PublicConfigResponse {
  study_name: string;
  /** Lifecycle state of the one experiment served by this process. */
  experiment_status: "inactive" | "active";
  /** Institution operating the study, when the server exposes one. */
  institution?: string | null;
  participant_information_version?: string | null;
  participant_information_url?: string | null;
  consents: ConsentItem[];
  voice?: {
    enabled?: boolean;
    transport?: "websocket";
    sample_rate_hz?: number;
    frame_duration_ms?: number;
    jitter_buffer_ms?: number;
  };
  transcription?: {
    enabled?: boolean;
    language?: string;
  };
  tts?: {
    enabled?: boolean;
    voice_name?: string | null;
  };
  agents?: {
    mode?: "human_vs_human" | "human_vs_agent" | string;
    human_vs_agent?: boolean;
  };
  privacy?: {
    contract_version?: string;
    store_full_game_state?: boolean;
    store_typed_messages?: boolean;
    store_final_transcripts?: boolean;
    store_voice_diagnostics?: boolean;
  };
}

export interface ConsentItem {
  id: string;
  title: string;
  /** Plain-text consent copy. */
  body: string;
  required: boolean;
}

export interface RoomResponse<TState = unknown, TObservation = TState, TAction = unknown, TEvent = unknown> {
  room_id: string;
  participant_session_id: string;
  role: "A" | "B" | "spectator" | string;
  presence?: Record<string, unknown>;
  state?: TState | null;
  observation?: TObservation | null;
  available_actions?: TAction[];
  events?: TEvent[];
  conversation?: ConversationMessage[];
}

export type AudioSinkPurpose = "partner-audio" | "transcription";

export interface AudioSessionPlan {
  enabled: boolean;
  websocket_url?: string | null;
  token?: string | null;
  protocol_version: number;
  sample_rate_hz: number;
  channels: number;
  frame_duration_ms: number;
  jitter_buffer_ms: number;
}

export interface GameSessionPlan {
  websocket_url: string;
  token: string;
}

export type ConversationOrigin = "typed" | "voice_transcript" | "agent" | "system";

export interface ConversationMessage {
  id: string;
  room_id: string;
  sender_participant_session_id?: string | null;
  sender_role?: string | null;
  text: string;
  origin: ConversationOrigin;
  source_message_id?: string | null;
  metadata?: Record<string, unknown>;
  created_at: string;
}

export type ServerMessage<
  TState = unknown,
  TObservation = TState,
  TAction = unknown,
  TEvent = unknown,
  TSummary = Record<string, unknown>
> =
  | {
      type: "roleAssigned";
      room_id: string;
      participant_session_id: string;
      role: "A" | "B" | "spectator" | string;
      state?: TState | null;
      observation?: TObservation | null;
      available_actions?: TAction[];
      events?: TEvent[];
      conversation?: ConversationMessage[];
    }
  | {
      type: "stateChanged";
      room_id: string;
      state?: TState | null;
      observation?: TObservation | null;
      available_actions?: TAction[];
      events?: TEvent[];
      conversation?: ConversationMessage[];
    }
  | { type: "conversationMessageAdded"; room_id: string; conversation_message: ConversationMessage }
  | { type: "completed"; room_id: string; summary: TSummary }
  | { type: "presenceChanged"; room_id?: string; presence?: Record<string, unknown> }
  | {
      type: "voiceStatusChanged";
      room_id?: string;
      voice?: {
        audioReady?: boolean;
        transcriptionReady?: boolean;
        transcriptionStatus?: string;
      };
    }
  | { type: "partnerWaiting"; room_id?: string; message?: string }
  | { type: "error"; room_id?: string; message?: string };

export function apiBase(): string {
  return window.location.origin;
}

export function socketUrl(websocketUrl: string, token: string): string {
  const base = new URL(websocketUrl);
  base.searchParams.set("token", token);
  return base.toString();
}

export class ExperimentApiClient {
  private participantCredential: string | null = null;

  constructor(private readonly baseUrl = apiBase()) {}

  getPublicConfig(): Promise<PublicConfigResponse> {
    return this.get("/api/config");
  }

  async createParticipant(): Promise<ParticipantCreateResponse> {
    const participant = await this.post<ParticipantCreateResponse>(
      "/api/participants",
      {}
    );
    this.participantCredential = participant.participant_credential;
    return participant;
  }

  submitConsent(decisions: Record<string, boolean>): Promise<void> {
    return this.postAuthenticated("/api/consent", { decisions });
  }

  createRoom<TState = unknown, TObservation = TState, TAction = unknown, TEvent = unknown>(): Promise<RoomResponse<TState, TObservation, TAction, TEvent>> {
    return this.postAuthenticated("/api/rooms", {});
  }

  getAudioSession(roomId: string): Promise<AudioSessionPlan> {
    return this.postAuthenticated(`/api/rooms/${roomId}/audio-session`, {});
  }

  getGameSession(roomId: string): Promise<GameSessionPlan> {
    return this.postAuthenticated(`/api/rooms/${roomId}/game-session`, {});
  }

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

  sendAction<TAction>(socket: WebSocket | null, action: TAction): void {
    socket?.send(JSON.stringify({ type: "submitAction", action }));
  }

  sendChatMessage(socket: WebSocket | null, text: string): void {
    socket?.send(JSON.stringify({ type: "sendChatMessage", text }));
  }

  socketUrl(plan: GameSessionPlan): string {
    return socketUrl(plan.websocket_url, plan.token);
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

export async function checkedJson<T>(responsePromise: Promise<Response> | Response): Promise<T> {
  const response = await responsePromise;
  if (!response.ok) {
    const detail = await response.text();
    throw new Error(detail || `Request failed with ${response.status}`);
  }
  return response.json();
}
