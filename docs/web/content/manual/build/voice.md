---
title: Configure communication
---

# Configure communication

Choose the communication channel as part of the condition. Check that participants can use it, that its storage settings match the study, and that the session record contains the evidence you need.

- **Typed conversation** lets participants exchange messages through the participant application. The game still controls legal actions and completion.
- **Human-to-human audio** relays microphone audio between participants. Test microphone permissions and reconnect behavior using the public HTTPS participant address.
- **Transcription and synthesized agent speech** use external services. Configure Speechmatics and ElevenLabs credentials in **Game settings** on a single-game installation started with `parlando serve`. The shared host accepts experiment configuration without provider credentials and currently has no dashboard for provisioning those keys. Use the single-game deployment when your study needs them.

Test the chosen channel in two independent browser profiles. Check mute controls and an ordinary session ending. If you enable transcription, check the stored final text and how it appears in a research export. Explain any external processing in your [participant information](../run/consent-privacy/).
