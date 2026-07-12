export interface ParticipantCreateResponse {
  participant_session_id: string;
  source: "direct" | "prolific" | "admin" | "test" | "agent";
  display_name?: string | null;
}

export interface PublicConfigResponse {
  study_name: string;
  require_consent: boolean;
  consents: ConsentItem[];
  livekit?: {
    enabled?: boolean;
    url?: string | null;
  };
  transcription?: {
    enabled?: boolean;
    provider?: string;
    language?: string;
    model?: string;
    store_audio?: boolean;
  };
  tts?: {
    enabled?: boolean;
    provider?: string;
    model?: string;
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

export interface LiveKitTokenResponse {
  enabled: boolean;
  url?: string | null;
  token?: string | null;
  identity?: string | null;
}

export type AudioSinkPurpose = "partner-audio" | "transcription";
export type AudioSinkTransport = "webrtc-room" | "websocket-stt" | "server-room-worker";

export interface AudioSinkPlan {
  id: string;
  provider: string;
  purposes: AudioSinkPurpose[];
  transport: AudioSinkTransport;
  credentials: Record<string, unknown>;
}

export interface AudioSessionPlan {
  enabled: boolean;
  capture: Record<string, unknown>;
  sinks: AudioSinkPlan[];
}

export interface TranscriptSegmentInput {
  participant_session_id: string;
  player: string;
  start_time_ms?: number;
  end_time_ms?: number;
  text: string;
  metadata?: Record<string, unknown>;
}

export interface TranscriptSegmentResponse extends TranscriptSegmentInput {
  id: string;
  room_id: string;
  move_count?: number | null;
  created_at: string;
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

export type ServerMessage<TState = unknown, TObservation = TState, TAction = unknown, TEvent = unknown> =
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
  | { type: "completed"; room_id: string; summary: Record<string, unknown> }
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

  getLiveKitToken(roomId: string, participantSessionId: string): Promise<LiveKitTokenResponse> {
    return this.post(`/api/rooms/${roomId}/livekit-token`, { participant_session_id: participantSessionId });
  }

  getAudioSession(roomId: string, participantSessionId: string): Promise<AudioSessionPlan> {
    return this.post(`/api/rooms/${roomId}/audio-session`, { participant_session_id: participantSessionId });
  }

  postTranscriptSegment(roomId: string, segment: TranscriptSegmentInput): Promise<TranscriptSegmentResponse> {
    return this.post(`/api/rooms/${roomId}/transcripts`, segment);
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
