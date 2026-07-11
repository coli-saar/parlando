import { Room, RoomEvent, Track } from "livekit-client";
import type { AudioSessionContext, LocalAudioSink, MicrophoneInput } from "./types";
import type { AudioSessionPlan, LiveKitTokenResponse } from "../protocol";

type PublishedAudioTrack = {
  trackSid: string;
  source: unknown;
  isMuted: boolean;
  mute(): Promise<unknown>;
  unmute(): Promise<unknown>;
};

interface LiveKitSinkOptions {
  id?: string;
  purposes?: ReadonlyArray<"partner-audio" | "transcription">;
  updateTranscriptionStatus?: boolean;
}

export class LiveKitCombinedSink implements LocalAudioSink {
  readonly id: string;
  readonly provider = "livekit";
  readonly purposes: ReadonlyArray<"partner-audio" | "transcription">;
  private readonly updateTranscriptionStatus: boolean;
  private room: Room | null = null;
  private publication: PublishedAudioTrack | null = null;
  private publishedInputTrack: MediaStreamTrack | null = null;
  private remoteAudio = new Map<string, HTMLAudioElement>();

  constructor(options: LiveKitSinkOptions = {}) {
    this.id = options.id ?? "livekit-combined";
    this.purposes = options.purposes ?? (["partner-audio", "transcription"] as const);
    this.updateTranscriptionStatus = options.updateTranscriptionStatus ?? this.purposes.includes("transcription");
  }

  async connect(input: MicrophoneInput, context: AudioSessionContext): Promise<void> {
    const token = await this.resolveCredentials(context);
    if (!token.enabled || !token.url || !token.token) {
      context.logVoice("voice_token_disabled");
      context.onVoiceStatus({
        connected: false,
        connecting: false,
        microphoneEnabled: false,
        remoteAudio: false,
        message: "Voice is disabled for this study",
        ...(this.updateTranscriptionStatus ? { transcriptionMessage: "ASR idle", transcriptionReady: true } : {})
      });
      return;
    }

    context.logVoice("voice_token_received", { identity: token.identity, livekit_url: token.url });
    const room = new Room();
    this.room = room;
    this.attachRoomDiagnostics(room, context);

    await room.connect(token.url, token.token);
    await room.startAudio().catch(() => {
      context.logVoice("start_audio_failed");
      context.onVoiceStatus({ message: "Click Join voice to allow audio playback" });
    });

    context.logVoice("publish_microphone_track_requested", { enabled: true });
    this.publication = await room.localParticipant.publishTrack(input.track, {
      source: Track.Source.Microphone
    }) as PublishedAudioTrack;
    this.publishedInputTrack = input.track;
    context.logVoice("publish_microphone_track_succeeded", {
      enabled: true,
      track_sid: this.publication.trackSid,
      source: this.publication.source,
      muted: this.publication.isMuted
    });
    context.onVoiceStatus({
      connected: true,
      connecting: false,
      microphoneEnabled: true,
      remoteAudio: this.remoteAudio.size > 0,
      message: this.remoteAudio.size > 0 ? "Voice connected" : "Microphone live",
      ...(this.updateTranscriptionStatus
        ? { transcriptionMessage: "Waiting for transcription service", transcriptionReady: false }
        : {})
    });
  }

  async setInputEnabled(enabled: boolean): Promise<void> {
    if (!this.publication) return;
    if (enabled) {
      await this.publication.unmute();
    } else {
      await this.publication.mute();
    }
  }

  async disconnect(): Promise<void> {
    if (this.room && this.publishedInputTrack) {
      await this.room.localParticipant.unpublishTrack(this.publishedInputTrack, false).catch(() => undefined);
    }
    this.publication = null;
    this.publishedInputTrack = null;
    this.room?.disconnect();
    this.room = null;
    this.clearRemoteAudio();
  }

  private async resolveCredentials(context: AudioSessionContext): Promise<LiveKitTokenResponse> {
    if (!context.getAudioSession) {
      return context.getLiveKitToken();
    }
    let plan: AudioSessionPlan;
    try {
      plan = await context.getAudioSession();
    } catch (error) {
      context.logVoice("audio_session_plan_failed", {
        error: error instanceof Error ? error.message : String(error)
      });
      return context.getLiveKitToken();
    }
    context.logVoice("audio_session_plan_received", {
      enabled: plan.enabled,
      sink_count: plan.sinks.length,
      sink_ids: plan.sinks.map((sink) => sink.id)
    });
    if (!plan.enabled) {
      return { enabled: false };
    }
    const sink = plan.sinks.find(
      (candidate) => candidate.provider === "livekit" && candidate.purposes.includes("partner-audio")
    );
    if (!sink) {
      context.logVoice("audio_session_livekit_sink_missing", {
        sink_ids: plan.sinks.map((candidate) => candidate.id)
      });
      return { enabled: false };
    }
    return {
      enabled: valueAsBoolean(sink.credentials.enabled, true),
      url: valueAsString(sink.credentials.url),
      token: valueAsString(sink.credentials.token),
      identity: valueAsString(sink.credentials.identity)
    };
  }

  private attachRoomDiagnostics(room: Room, context: AudioSessionContext): void {
    room.on(RoomEvent.Connected, () => {
      context.logVoice("livekit_connected", {
        identity: room.localParticipant.identity,
        connection_state: String(room.state)
      });
    });
    room.on(RoomEvent.ConnectionStateChanged, (state) => {
      context.logVoice("livekit_connection_state_changed", { state: String(state) });
    });
    room.on(RoomEvent.Reconnecting, () => context.logVoice("livekit_reconnecting"));
    room.on(RoomEvent.Reconnected, () => context.logVoice("livekit_reconnected"));
    room.on(RoomEvent.ParticipantConnected, (participant) => {
      context.logVoice("remote_participant_connected", { participant_identity: participant.identity });
    });
    room.on(RoomEvent.ParticipantDisconnected, (participant) => {
      context.logVoice("remote_participant_disconnected", { participant_identity: participant.identity });
    });
    room.on(RoomEvent.TrackPublished, (publication, participant) => {
      context.logVoice("remote_track_published", {
        participant_identity: participant.identity,
        track_sid: publication.trackSid,
        source: publication.source,
        kind: publication.kind
      });
    });
    room.on(RoomEvent.TrackSubscribed, (track, publication, participant) => {
      context.logVoice("remote_track_subscribed", {
        participant_identity: participant.identity,
        track_sid: publication.trackSid,
        source: publication.source,
        kind: track.kind
      });
      if (track.kind !== Track.Kind.Audio) return;
      const element = track.attach();
      element.autoplay = true;
      element.dataset.participantIdentity = participant.identity;
      document.body.appendChild(element);
      this.remoteAudio.set(publication.trackSid, element);
      context.onVoiceStatus({ remoteAudio: true, message: "Voice connected" });
      element
        .play()
        .then(() => {
          context.logVoice("remote_audio_playback_started", {
            participant_identity: participant.identity,
            track_sid: publication.trackSid
          });
        })
        .catch((error) => {
          context.logVoice("remote_audio_playback_failed", {
            participant_identity: participant.identity,
            track_sid: publication.trackSid,
            error: error instanceof Error ? error.message : String(error)
          });
          context.onVoiceStatus({ message: "Click Join voice to allow audio playback" });
        });
    });
    room.on(RoomEvent.TrackSubscriptionFailed, (trackSid, participant) => {
      context.logVoice("remote_track_subscription_failed", {
        participant_identity: participant.identity,
        track_sid: trackSid
      });
    });
    room.on(RoomEvent.TrackUnsubscribed, (_track, publication) => {
      context.logVoice("remote_track_unsubscribed", { track_sid: publication.trackSid, source: publication.source });
      const element = this.remoteAudio.get(publication.trackSid);
      if (element) {
        element.remove();
        this.remoteAudio.delete(publication.trackSid);
      }
      context.onVoiceStatus({
        remoteAudio: this.remoteAudio.size > 0,
        message: this.remoteAudio.size > 0 ? "Voice connected" : "Microphone live"
      });
    });
    room.on(RoomEvent.Disconnected, () => {
      context.logVoice("livekit_disconnected");
      this.clearRemoteAudio();
      this.room = null;
      this.publication = null;
      context.onVoiceStatus({
        connected: false,
        connecting: false,
        microphoneEnabled: false,
        remoteAudio: false,
        message: "Voice disconnected",
        ...(this.updateTranscriptionStatus ? { transcriptionMessage: "ASR idle", transcriptionReady: false } : {})
      });
    });
    room.on(RoomEvent.AudioPlaybackStatusChanged, (playing) => {
      context.logVoice("audio_playback_status_changed", { playing });
      context.onVoiceStatus({
        message: playing
          ? this.remoteAudio.size > 0
            ? "Voice connected"
            : "Microphone live"
          : "Click Join voice to allow audio playback"
      });
    });
    room.on(RoomEvent.LocalTrackPublished, (publication) => {
      context.logVoice("local_track_published", {
        track_sid: publication.trackSid,
        source: publication.source,
        kind: publication.kind
      });
    });
    room.on(RoomEvent.LocalTrackSubscribed, (publication) => {
      context.logVoice("local_track_subscribed_by_remote", {
        track_sid: publication.trackSid,
        source: publication.source,
        kind: publication.kind
      });
    });
    room.on(RoomEvent.LocalAudioSilenceDetected, () => {
      context.logVoice("local_audio_silence_detected");
    });
  }

  private clearRemoteAudio(): void {
    for (const element of this.remoteAudio.values()) {
      element.remove();
    }
    this.remoteAudio.clear();
  }
}

export class LiveKitPartnerAudioSink extends LiveKitCombinedSink {
  constructor() {
    super({
      id: "livekit-partner",
      purposes: ["partner-audio"],
      updateTranscriptionStatus: false
    });
  }
}

function valueAsString(value: unknown): string | null {
  return typeof value === "string" ? value : null;
}

function valueAsBoolean(value: unknown, fallback: boolean): boolean {
  return typeof value === "boolean" ? value : fallback;
}
