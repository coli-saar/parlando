---
title: Configure communication
---

# Configure communication

Choose **Typed** or **Speech** in the experiment's communication settings. The choice applies to both seats and determines which interface the participant application displays. You can use the same game package for both conditions.

| Mode | Participant interface | Operator setup |
| --- | --- | --- |
| Typed (default) | Message history, text field, and Send button | Speech settings are disabled. Participants can enter without microphone permission. Speechmatics and ElevenLabs stay inactive, including when the experiment retains settings for them. |
| Speech | Microphone level, status, and mute control | Use HTTPS for participant microphone access. Configure transcription to deliver finalized human utterances to an agent and synthesis to make its replies audible. |

In experiment configuration files the field is `communication.mode`, with values `typed` and `speech`. For example:

```yaml
communication:
  mode: speech
```

Select **Speech** to edit audio transport, Speechmatics transcription, and ElevenLabs synthesis settings. Switching to **Typed** preserves those values for the next time you select **Speech**. Provider credentials belong in **Game settings** on a single-game installation started with `parlando serve`. The shared host currently has no dashboard for provisioning provider keys.

Pilot the chosen mode in two independent browser profiles. For typed sessions, exchange a message in each direction. For speech, check microphone permission, mute, partner playback, and reconnect using the public participant address. If transcription is enabled, check the final text in a research export. Explain any external processing in your [participant information](../run/consent-privacy/).
