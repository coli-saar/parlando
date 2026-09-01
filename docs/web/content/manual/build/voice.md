---
title: Enable typed or spoken dialogue
description: Add a typed message interface, or configure and pilot Parlando's supported human–human and human–agent speech paths.
---

# Enable typed or spoken dialogue

Consider one turn in a spoken Great Tree session. A participant explains what they can see. Their
browser sends microphone audio to Parlando, which relays the speech to the other human or sends it
to Speechmatics for transcription. In a human–agent session, the final transcript becomes the
message that the agent receives. When the agent replies, Parlando stores the reply as text and uses
ElevenLabs to make it audible to the participant.

This chapter documents the communication interfaces that Parlando 0.4.0 supports as complete study
paths. It does not treat every combination of low-level configuration switches as a separate
condition.

{{< figure class="architecture-figure" src="manual/images/parlando-speech-paths.svg" alt="The three documented Parlando communication paths: a game-authored typed interface, transcribed human–human speech, and transcribed human–agent speech with a synthesized agent reply" caption="Typed dialogue is implemented in the game interface. The two documented speech paths use transcription; human–agent speech also synthesizes the agent's stored reply." >}}

## Choose one documented communication interface

Parlando 0.4.0 supports the following interfaces:

| Interface | What you provide or enable | What Parlando records |
| --- | --- | --- |
| Typed dialogue | A message display and text input in the game's React interface | Sent message text |
| Human–human speech | Voice transport and Speechmatics transcription | Final transcript text and utterance timing; no raw audio |
| Human–agent speech | Voice transport, Speechmatics transcription, a human–agent experiment, and ElevenLabs text-to-speech | Final human transcripts and agent message text; no raw or synthesized audio |

There is no generic chat widget in the Parlando 0.4.0 React package. The client package supports
typed messages, but the game author must decide how to display the conversation and must provide the
text-entry control. Likewise, this manual does not define a voice-without-transcription condition or
text-only variants of the spoken human–agent path. The fact that the configuration model exposes
several technical switches does not by itself make every switch combination a supported study
interface.

## Add typed dialogue to a game

Typed dialogue belongs to the game-specific React application, not to the experiment dashboard.
The `GameSession` supplied to `renderGame` contains the current `conversation` and a
`sendMessage(text)` method. Render the conversation, collect a non-empty string from the
participant, and pass it to that method:

```tsx
function GreatTreeGame({ session }: { session: GreatTreeSession }) {
  const [draft, setDraft] = useState("");

  function sendMessage(event: FormEvent) {
    event.preventDefault();
    const text = draft.trim();
    if (!text || !session.interactionEnabled) return;
    session.sendMessage(text);
    setDraft("");
  }

  return (
    <>
      <ol aria-label="Conversation">
        {session.conversation.map((message) => (
          <li key={message.id}>{message.text}</li>
        ))}
      </ol>
      <form onSubmit={sendMessage}>
        <label htmlFor="message">Message</label>
        <input
          id="message"
          value={draft}
          onChange={(event) => setDraft(event.target.value)}
          disabled={!session.interactionEnabled}
        />
        <button disabled={!session.interactionEnabled || !draft.trim()}>Send</button>
      </form>
    </>
  );
}
```

The example is deliberately plain. A production interface should also distinguish the two
speakers, keep the newest message visible, announce incoming messages accessibly, and explain
whether the other seat is occupied by a person or an agent. Test the control with keyboard-only
navigation and disable it whenever `interactionEnabled` is false.

A message communicates language to the other seat; it does not change authoritative game state.
For example, “I think we should tend the eastern branch” is a message, whereas choosing the eastern
branch and pressing **Tend** sends a game action. Keep task decisions as structured actions even if
participants also discuss them in messages. This preserves the distinction between what players
said and what the game accepted.

## Configure human–human speech

In the documented human–human speech path, each participant hears the other's live microphone
audio and Speechmatics produces the final transcript record.

1. Sign in at `/admin`, open **Game settings**, and enter the Speechmatics API key. Keep the default
   endpoint unless your provider agreement specifies a different endpoint. Select **Save all game
   settings**.
2. Open the inactive experiment's **Configuration** tab and select human–human pairing.
3. Enable **Voice transport** and keep the default playback setting for the first pilot.
4. Enable **Speech recognition**. Choose the language and model, then review the transcript delay
   and utterance-ending silence.
5. Under **Capacity**, reserve two transcription streams for every simultaneously active session:
   one for each human microphone.
6. Save the revision and open **Privacy**. Confirm that it names Speechmatics as a recipient of live
   microphone audio and final transcripts as retained research data.

{{< figure src="manual/images/game-settings.png" alt="Game settings page with the Speechmatics endpoint and API-key control" caption="Speech provider credentials belong to the installation. They are not copied into experiment revisions, browser assets, or research exports." >}}

{{< figure src="manual/images/experiment-speech.png" alt="Experiment configuration showing enabled voice transport and speech recognition" caption="The documented human–human speech path enables both voice transport and recognition. Parlando relays live audio and stores final transcript messages, but not raw audio." >}}

Start **Local preview** in two isolated browser contexts. In each context, select **Prepare voice**,
grant microphone access, choose the intended input, and speak while watching the input meter. Admit
both participants, confirm that each hears the other, and then inspect the conversation or export to
confirm that both final utterances have the correct speaker and timing. Repeat this pilot through
the public HTTPS origin before recruitment.

## Configure human–agent speech

In the documented human–agent speech path, the human occupies seat `A` and the selected agent
occupies seat `B`. The human speaks; Speechmatics turns each final utterance into a conversation
message for the agent. The agent replies with text; Parlando stores that text before ElevenLabs
synthesizes it for the participant.

1. Complete the Speechmatics settings described above. Under **Game settings**, also enter the
   ElevenLabs API key.
2. Open the inactive experiment's **Configuration** tab. Choose human–agent pairing and select the
   tested agent implementation.
3. Enable **Voice transport** and **Speech recognition** with the intended language and recognition
   settings.
4. Enable **Text to speech**. Choose the model, enter the stable ElevenLabs voice ID, and provide a
   participant-facing voice name.
5. Reserve one transcription stream for each simultaneously active human–agent session.
6. Save the revision. On **Privacy**, confirm that Speechmatics receives human microphone audio and
   ElevenLabs receives agent-authored text plus the configured synthesis parameters.

Pilot one complete turn. Speak a short task-relevant utterance, verify that its final transcript
enters the conversation, verify that the agent receives it and returns a message, and listen for the
synthesized reply. Inspect the session record as well as the participant interface. A reply that is
audible but absent from the record, or present in the record but not audible, reveals two different
failures.

The Speechmatics setting **Show partial transcripts** asks the provider to produce provisional
results. Parlando 0.4.0 neither displays nor stores those partial hypotheses. Only final utterances
enter the conversation, so do not design participant feedback around word-by-word captions.

## Provide the in-game speech controls

The standard participant startup flow handles microphone preparation before room entry. A
participant selects **Prepare voice**, grants browser permission, chooses an input when necessary,
and sees a level check. The waiting display then reports participant and transcription readiness.

The game's React interface remains responsible for controls needed during play. Expose microphone
state and call `setMicrophoneMuted(muted)` from a clearly labelled mute control. The session also
reports `voiceEnabled`, `voiceStatus`, and voice-preflight state. Muting stops outgoing capture but
does not stop incoming partner or agent playback.

Do not make the interface imply functionality that the experiment does not provide. Tell
participants whether the other seat contains a person or an agent, that live microphone audio is
being processed, that final transcripts are retained, and—in a human–agent session—that the agent's
reply uses a synthesized voice.

## Understand what leaves Parlando and what remains

| Data | Destination | Durable Parlando record |
| --- | --- | --- |
| Typed participant message | Parlando and the other seat | Message text |
| Human microphone audio in a speech session | Parlando; the other browser in human–human sessions; Speechmatics | No raw audio |
| Final Speechmatics utterance | Parlando and the other seat or agent | Transcript text, speaker, and timing |
| Interim recognition hypothesis | Returned by Speechmatics to Parlando | Neither displayed nor stored |
| Agent-authored message | Parlando and the human participant | Message text |
| Agent text submitted for synthesis | ElevenLabs with voice and model parameters | The agent message is already stored; generated audio is not stored |
| Game action | Parlando and the authoritative game module | Accepted action and resulting state |

ElevenLabs does not receive participant microphone audio, participant transcripts, identifiers, or
game state through Parlando's synthesis path. Speechmatics does receive live participant microphone
audio. Provider-side logging and retention are governed by the institution's agreements and
provider settings, not by Parlando's database behavior.

Participant information should name the providers, their purposes, and applicable processing
regions. It should explain that final transcripts may contain identifying information even though
Parlando does not ask for a participant's name. Ask participants to discuss only the task and
review free dialogue before releasing a corpus. Adapt the
[participant information](../privacy/participant-information-v1.0/) to the actual deployment before
using the corresponding consent item.

## Pilot failures before recruitment

Speech readiness requires more than a healthy game page. Each human browser needs microphone
permission and an active audio connection, and Speechmatics must be ready before the session can
start. Test the following cases in the same browsers and networks that participants will use:

- denied microphone permission and the wrong input device;
- mute, unmute, ordinary room noise, and quiet speakers;
- network disconnection and reconnection;
- Speechmatics delay or failure;
- agent timeout in a human–agent session; and
- ElevenLabs delay or failure in a human–agent session.

If transcription fails, the final dialogue record is incomplete and an agent cannot understand new
speech. If synthesis fails, the stored agent message still exists, although the participant does not
hear it. Decide before recruitment which failures end a session, how participants will be informed,
and how incomplete sessions will be compensated and analyzed.

Production speech requires HTTPS and one Parlando server process. The included deployment patterns
do not support multiple replicas for a live study.

The stress tests exercise Parlando under sustained speech traffic without paid provider calls. They
do not test a participant browser, public deployment, actual provider account, or participant
network. Run both the stress test and a complete public-origin pilot before recruitment.

Parlando 0.4.0 includes Speechmatics transcription and ElevenLabs synthesis, stores neither raw
microphone nor synthesized audio, provides no built-in local recognizer, and has no speech path in a
headless agent–agent run. These are current product boundaries, not a catalogue of hypothetical
conditions.

The next part turns the game, agent, and communication interface into a versioned study condition,
beginning with [Configure an experiment](../run/configuration/).
