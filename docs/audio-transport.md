# Audio Transport

Parlando provides a small server-owned audio transport for two-player dialogue games. It carries live microphone audio between browsers, supplies the same incoming stream to server-side speech recognition, and returns synthesized agent speech to browsers. Game projects consume this capability through `parlando-server` and `@coli-saar/parlando-client`; they do not implement media transport themselves.

## Data Flow

Each human browser opens one authenticated bidirectional WebSocket:

```text
Browser A microphone ─┬─> Parlando room relay ─> Browser B playback
                      └─> TranscriptionProvider ─> conversation and agents

AgentResponse.message ─> TTS provider ─> Parlando room relay ─> browser playback
```

Partner relay, transcription, and agent playback are independent consumers. A slow or failed transcription provider does not block partner audio. Synthesized agent audio enters the outbound room relay directly and is not transcribed again.

## Session And Authentication

After joining a room, the browser requests `POST /api/rooms/{room_id}/audio-session` with its participant-session id. When voice is enabled, the response contains:

- the `/ws/audio/{room_id}` URL;
- an opaque, random token valid for five minutes and one WebSocket upgrade;
- protocol version, sample rate, channel count, frame duration, and jitter-buffer target.

Token claims remain in server memory. The token is bound to the current room, participant session, and authoritative role, and it is removed when used. Production deployments must use HTTPS/WSS so microphone audio and the query token are encrypted in transit. Avoid query-string logging at reverse proxies even though the token contains no readable participant data.

## Version 1 Wire Format

Version 1 uses raw signed 16-bit little-endian PCM at 24 kHz, mono, in 20 ms frames. A binary WebSocket message is exactly 973 bytes:

| Offset | Size | Encoding | Meaning |
| --- | ---: | --- | --- |
| 0 | 1 byte | unsigned | protocol version `1` |
| 1 | 4 bytes | big-endian `u32` | connection-local sequence number |
| 5 | 8 bytes | big-endian `u64` | capture timestamp in milliseconds |
| 13 | 960 bytes | PCM16LE | 480 mono samples, or 20 ms |

One direction uses about 49 kB/s before WebSocket/TLS overhead. Raw PCM is deliberately simple and observable, but less bandwidth-efficient than a compressed codec and subject to TCP head-of-line blocking.

The same binary shape is used in both directions. Browsers may therefore receive partner microphone frames and server-generated agent frames through the same playback queue. JSON text messages on the socket carry control state such as transcription readiness or errors.

## Browser Capture And Playback

`ParlandoAudioSink` owns browser media setup:

- a capture AudioWorklet converts the selected microphone to 24 kHz PCM16 frames;
- the WebSocket sends complete frames only while the microphone is enabled;
- a playback AudioWorklet converts 24 kHz PCM to the output-device rate with linear interpolation;
- worklet entry modules are self-contained so consumer bundlers can emit them as standalone assets or data/blob modules;
- playback starts after the configured jitter target, normally 100 ms;
- after an underrun, playback resumes after a smaller 40 ms recovery buffer;
- very stale buffered audio is discarded so latency cannot grow without bound;
- every playback underrun is recorded as an `audio_playback_underrun` voice diagnostic.

Microphone preparation happens before room entry. Waiting-room startup reuses that prepared input for one audio-transport connection attempt; a WebSocket or worklet failure cleans up only the partial transport and never reacquires the microphone in a retry loop.

Agent TTS is finite audio rather than a naturally clocked microphone. The server sends the initial jitter-buffer window immediately, then schedules later frames against absolute 20 ms deadlines. Absolute deadlines prevent per-frame processing and timer overhead from accumulating until the browser buffer runs dry.

## Transcription Boundary

The server creates one `TranscriptionProvider` session per human microphone stream. Providers receive canonical `AudioFrame` values and emit provider-neutral events:

- `Ready`;
- replaceable `Partial` text for status displays;
- `FinalUtterance` with text, timing, and stable result ids;
- `Failed`.

Only final utterances are durable. Parlando deduplicates provider results, persists a transcript event, adds a `voice_transcript` conversation message, broadcasts it on the game WebSocket, and calls `GameAgent::observe_message` with spoken modality. Speechmatics is the current hosted implementation and is contacted only by the server. Its API key is never returned to a browser. A future local recognizer can implement the same provider trait without changing browser or agent code.

Raw audio is not persisted by the version 1 relay. Speechmatics still receives microphone audio when configured, so studies that require audio to remain entirely on their own infrastructure must use a future local provider.

## TTS Boundary

Agents speak by returning `AgentResponse.message`. Parlando persists that text first, asks the configured streaming TTS provider for 24 kHz mono PCM, and publishes the resulting frames through the room relay. Game adapters and browser clients must not call a TTS service or publish audio directly.

## Deployment And Scaling

Active audio rooms, one-use tokens, and playback queues are process-local. Run one server process or configure sticky routing so all HTTP and WebSocket requests for a room reach the same instance. A future multi-instance deployment needs a shared room/media backplane rather than ordinary stateless load balancing.

Bounded queues protect the game server from slow clients. When an outbound queue is full, live frames may be dropped; the browser buffer is designed to recover and report an underrun instead of allowing unbounded latency. Monitor voice diagnostics and WebSocket close/error rates during real studies.

## Configuration

```yaml
voice:
  enabled: true
  sample_rate_hz: 24000
  frame_duration_ms: 20
  jitter_buffer_ms: 100

speechmatics:
  api_key: ${SPEECHMATICS_API_KEY}
  realtime_url: wss://eu.rt.speechmatics.com/v2
  max_delay: 2.0
  enable_partials: true
  end_of_utterance_silence_trigger: 1.2

transcription:
  enabled: true
  provider: speechmatics
  model: enhanced
  language: en
  store_audio: false

tts:
  enabled: true
  provider: elevenlabs
  model: eleven_flash_v2_5
  voice_id: ${ELEVENLABS_VOICE_ID}
  api_key: ${ELEVENLABS_API_KEY}
  output_format: pcm_24000
```

Protocol version 1 requires `sample_rate_hz: 24000`, `frame_duration_ms: 20`, mono PCM, and a TTS output format compatible with 24 kHz PCM.
