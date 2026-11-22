---
title: Configure and run a study
---

# Configure and run a study

Use a [local pilot](../start/getting-started/) to verify the participant procedure, then create a run on the host that participants will use. Each hosted experiment has a unique ID and pins an exact package revision and configuration. Choose a fresh ID for a different package or condition.

## Choose the condition

In the hosted dashboard, open **Create an experiment**, choose a game revision, and set **Communication mode**. The default **Typed** condition provides text chat. **Speech** enables the speech settings. Use **Other experiment settings (JSON)** for settings such as pairing, timing, and game parameters. Leaving this object empty uses direct entry with a human partner. To reuse an existing setup, choose **Copy configuration to new experiment** and give the copy a fresh ID. The local single-game dashboard also provides a communication selector. To supply the complete configuration as a file, pass `--config experiment.yaml` to `parlando select`.

Before opening a study, settle the following decisions for that condition.

- **Participant procedure.** Check the instructions, consent, entry link, and what participants see on completion.
- **Pairing and timing.** Check two independent entries, partner waiting, disconnection, and the end of a session.
- **Data collection.** Decide which conversation data you need, how long to keep it, and what you will export. Review the [consent and privacy guide](consent-privacy/).
- **Optional integrations.** Configure credentials in a supported deployment before relying on speech services or Prolific. See the [speech guide](../build/voice/) and [Prolific guide](prolific/).

For example, set a two-minute partner wait and a 20-minute maximum session
for a hosted Beacon run. Other fields use their defaults.

```json
{
  "session": {
    "waiting_session_timeout_seconds": 120,
    "session_idle_timeout_seconds": 600,
    "session_max_lifetime_seconds": 1200
  },
  "game": {}
}
```

Game-specific settings belong under `game` and must match the chosen package's game schema. Beacon Relay needs an empty game configuration. Keep provider credentials out of the experiment JSON or YAML.

## Open and close intake

| Intake state | Use it for |
| --- | --- |
| Inactive | Keep the condition closed to new participants. |
| Testing | Run a pilot. Testing sessions appear in monitoring and are excluded from research corpus exports. |
| Active | Admit research participants under the selected condition. |

Play both seats in separate browser profiles during the pilot. Verify a successful completion and what happens when a participant leaves. Check the resulting session in the dashboard. For actual data collection, use Active and give participants the entry URL from that experiment. Keep track of the game revision, experiment ID, and recruitment link together.

Close intake when recruitment ends. Keep the host running while participants finish. On restart, reopen intake explicitly. See [monitor sessions and export results](data-operations/) before archiving a study.
