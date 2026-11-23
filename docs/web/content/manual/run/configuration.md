---
title: Configure and run a study
---

# Configure and run a study

Use a [local pilot](../start/getting-started/) to verify the participant procedure, then create a run on the host that participants will use. Each game has a catalogue of experiments. Use a separate experiment ID for each condition that runs independently.

## Choose the condition

Activate the deployed game with `parlando activate --host SERVER_URL --game GAME`. In **Games**, click its name to open its dashboard. Choose **New experiment**, enter an ID such as `Beacon-Test`, and choose **Create experiment**. Use 1–128 ASCII letters, digits, dots, dashes, or underscores. The experiment starts inactive. Its ID is scoped to the game and is part of its participant URL, such as `/e/beacon~Beacon-Test/` on the participant origin.

Select the experiment and open **Configuration**. **Communication mode** defaults to **Typed**. Select **Speech** to edit speech settings. For a Beacon run with a two-minute partner wait and 20-minute maximum session, enter `120` in **Waiting-session timeout (seconds)** and `1200` in **Maximum lifetime (seconds)** under **Session lifecycle**. Choose **Save new revision** while the experiment is inactive. Use **Clone** to copy an inactive condition to a new experiment ID.

Enter game-specific values as YAML under **Game configuration**. The dashboard validates them against the current game package. Beacon Relay uses an empty game configuration. Store named game credentials under **Game secrets** and provider credentials in **Game settings**.

Before opening a study, verify the following for the condition you plan to run.

- **Participant procedure.** Check the instructions, consent, entry link, and what participants see on completion.
- **Pairing and timing.** Check two independent entries, partner waiting, disconnection, and the end of a session.
- **Data collection.** Decide which conversation data you need, how long to keep it, and what you will export. Review the [consent and privacy guide](consent-privacy/).
- **Optional integrations.** Configure credentials before relying on speech services or Prolific. See the [speech guide](../build/voice/) and [Prolific guide](prolific/).

## Open and close intake

| Intake state | Use it for |
| --- | --- |
| Inactive | Keep the condition closed to new participants. |
| Testing | Run a pilot. Testing sessions appear in monitoring and are excluded from research corpus exports. |
| Active | Admit research participants under the selected condition. |

Open **Start experiment** and start **Local preview** for the pilot. Play both seats in separate browser profiles and check the resulting sessions in the dashboard. Use **Official intake** for data collection and distribute the URL from the running experiment. Record the game package revision, experiment ID, and recruitment link together.

Use **Pause intake** when recruitment ends. Keep the host running while participants finish. On restart, reopen intake explicitly. See [monitor sessions and export results](data-operations/) before archiving a study.
