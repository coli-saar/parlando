# Live-service tests

Speechmatics and ElevenLabs checks use real credentials and may consume paid API resources. They stay ignored during normal `cargo test` runs.

Use a private voice config containing server-side provider credentials:

```sh
PARLANDO_RUN_LIVE_TESTS=1 \
PARLANDO_LIVE_CONFIG=config/experiment.voice.private.yaml \
cargo test --test live_services -- --ignored --nocapture
```

The Speechmatics smoke test opens a realtime transcription session through the production server-side provider. Browser clients never receive its API key and never connect to Speechmatics directly.

The in-process audio relay, frame validation, partner routing, and agent-audio routing are covered by ordinary unit/integration tests and require no third-party media service.
