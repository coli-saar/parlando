export interface ParticipantCreateResponse {
  participant_session_id: string;
  source: "direct" | "prolific" | "admin" | "test" | "agent";
  display_name?: string | null;
}

export interface PublicConfigResponse {
  study_name: string;
  require_consent: boolean;
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
    store_audio?: boolean;
  };
  tts?: {
    enabled?: boolean;
    voice_name?: string | null;
    worker_autostart?: boolean;
  };
  conversation?: {
    enabled?: boolean;
    max_history_messages?: number;
  };
  agents?: {
    mode?: "human_vs_human" | "human_vs_agent" | string;
    human_vs_agent?: boolean;
  };
}

export interface ConsentItem {
  id: string;
  title: string;
  body_html: string;
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

export type RoomMode = "direct" | string;

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

export function socketUrl(roomId: string, participantSessionId: string, baseUrl = apiBase()): string {
  const base = new URL(baseUrl);
  base.protocol = base.protocol === "https:" ? "wss:" : "ws:";
  base.pathname = `/ws/game/${roomId}`;
  base.search = new URLSearchParams({ participantSessionId }).toString();
  return base.toString();
}

export class ExperimentApiClient {
  constructor(private readonly baseUrl = apiBase()) {}

  getPublicConfig(): Promise<PublicConfigResponse> {
    return this.get("/api/config");
  }

  createParticipant(displayName?: string): Promise<ParticipantCreateResponse> {
    return this.post("/api/participants", { source: "direct", display_name: displayName || null });
  }

  submitConsent(participantSessionId: string, decisions: Record<string, boolean>): Promise<void> {
    return this.post("/api/consent", { participant_session_id: participantSessionId, decisions });
  }

  createRoom<TState = unknown, TObservation = TState, TAction = unknown, TEvent = unknown>(
    participantSessionId: string,
    mode: RoomMode = "direct"
  ): Promise<RoomResponse<TState, TObservation, TAction, TEvent>> {
    return this.post("/api/rooms", { participant_session_id: participantSessionId, mode });
  }

  joinRoom<TState = unknown, TObservation = TState, TAction = unknown, TEvent = unknown>(
    roomId: string,
    participantSessionId: string
  ): Promise<RoomResponse<TState, TObservation, TAction, TEvent>> {
    return this.post(`/api/rooms/${roomId}/join`, { participant_session_id: participantSessionId });
  }

  getAudioSession(roomId: string, participantSessionId: string): Promise<AudioSessionPlan> {
    return this.post(`/api/rooms/${roomId}/audio-session`, { participant_session_id: participantSessionId });
  }

  postVoiceDiagnostic(
    roomId: string,
    participantSessionId: string,
    event: string,
    metadata: Record<string, unknown> = {}
  ): void {
    void fetch(`${this.baseUrl}/api/rooms/${roomId}/voice-diagnostics`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        participant_session_id: participantSessionId,
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

  socketUrl(roomId: string, participantSessionId: string): string {
    return socketUrl(roomId, participantSessionId, this.baseUrl);
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
}

export async function checkedJson<T>(responsePromise: Promise<Response> | Response): Promise<T> {
  const response = await responsePromise;
  if (!response.ok) {
    const detail = await response.text();
    throw new Error(detail || `Request failed with ${response.status}`);
  }
  return response.json();
}
