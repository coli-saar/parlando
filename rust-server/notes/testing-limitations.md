# Testing limitations

Ordinary `cargo test` covers the fixed PCM frame contract, token authorization and replay rejection, simultaneous-room isolation, replacement connections, short-lived audio-session plans, a local Speechmatics protocol contract, server-side transcription normalization, transcript/conversation persistence, agent observations, and TTS publishing. It requires no service credentials and makes no paid provider calls.

The remaining production-confidence checks are:

- audible two-browser verification across realistic network conditions;
- sustained backpressure testing under packet delay;
- Linux/deployment validation for long-running audio WebSockets;
- later validation of a local STT provider through the same `TranscriptionProvider` boundary.

See `docs/audio-testing.md` for the normal test commands and standalone stress dashboard. Real Speechmatics is deliberately excluded from repository tests.

The transport is a process-local relay. Multi-instance deployment therefore requires sticky room routing or a future shared media backplane.
