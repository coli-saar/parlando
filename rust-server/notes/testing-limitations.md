# Testing limitations

Automated tests cover the fixed PCM frame contract, room routing, short-lived audio-session plans, server-side transcription normalization, transcript/conversation persistence, agent observations, TTS publishing, browser protocol helpers, and browser build.

The remaining production-confidence checks are:

- audible two-browser verification across realistic network conditions;
- reconnection and sustained backpressure testing under packet delay;
- a real Speechmatics utterance completing through `/ws/audio/{room_id}` into storage and an agent;
- Linux/deployment validation for long-running audio WebSockets;
- later validation of a local STT provider through the same `TranscriptionProvider` boundary.

The transport is a process-local relay. Multi-instance deployment therefore requires sticky room routing or a future shared media backplane.
