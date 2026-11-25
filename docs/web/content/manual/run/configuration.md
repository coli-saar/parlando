---
title: Configure and run a study
---

# Configure and run a study

Use a [local pilot](../start/getting-started/) to verify the participant procedure, then create a run on the host that participants will use. Each game has a catalogue of experiments. Use a separate experiment ID for each condition that runs independently.

## Choose the condition

In **Games**, click its name to open its dashboard. Choose **New experiment**, enter an ID such as `Beacon-Test`, and choose **Create experiment**. Use 1–128 ASCII letters, digits, dots, dashes, or underscores. The experiment starts inactive. Its ID is scoped to the game and is part of its participant URL, such as `/e/beacon~Beacon-Test/` on the participant origin.

Select the experiment and open **Configuration**. **Communication mode** defaults to **Typed**. Select **Speech** to edit speech settings. For a Beacon run with a two-minute partner wait and 20-minute maximum session, enter `120` in **Waiting-session timeout (seconds)** and `1200` in **Maximum lifetime (seconds)** under **Session lifecycle**. Choose **Save new revision** while the experiment is inactive. Use **Clone** to copy an inactive condition to a new experiment ID.

Enter game-specific values as YAML under **Game configuration**. The dashboard validates them against the current game package. Beacon Relay uses an empty game configuration. Store named game credentials under **Game secrets** and provider credentials in **Game settings**.

Before opening a study, verify the following for the condition you plan to run.

- **Participant procedure.** Check the instructions, consent, entry link, and what participants see on completion.
- **Pairing and timing.** Check two independent entries, partner waiting, disconnection, and the end of a session.
- **Data collection.** Decide which conversation data you need, how long to keep it, and what you will export. Review the [consent and privacy guide](consent-privacy/).
- **Optional integrations.** Configure credentials before relying on speech services or Prolific. See the [speech guide](../build/voice/) and [Prolific guide](prolific/).

## Configure the public imprint

Under **Participant access and consent → Imprint**, select the country where the
study operator is established. The selector provides templates for Germany, the
United States, France, Austria, Spain, Ireland, Italy, and the Netherlands, plus a
general EU starting point. Each generated imprint names the selected template and
lists its legal references with official source URLs, qualified by applicability
to the operator. Standalone HTTPS source URLs are clickable in the preview, public
page, and Markdown download. The templates omit German and Austrian media-law references. The US
template explains the absence of a general federal imprint requirement. The EU
option does not replace national requirements. Existing saved text is preserved;
to use an updated template, select another country and then the intended country
again, which replaces the current text.

The operating institution is pre-filled from **Shared settings → Institution**.
Enter the responsible contact person's name, public postal address, email, and
telephone. These fields populate the template; they do not identify the contact
person as the institution's legal representative. Complete the remaining
placeholders and remove inapplicable lines. The templates assume a public
annotation study without selling or editorial publishing and require review for
the operator's legal form and applicable national requirements.

Contact changes update an unedited template. Once you customize the text, contact
changes preserve it. Selecting a different country replaces the text with that
country’s template, using the current contact fields.

**Privacy → Imprint** displays the current imprint immediately, including unsaved
edits. A draft label distinguishes unsaved text from the saved public notice.

**Save new revision** publishes the exact imprint text at the study's
`information/imprint/` URL, including while intake is closed. **Privacy → Imprint**
provides the public link and a Markdown download of the same notice. Changes to shared
institution settings do not rewrite an already saved notice. Keep a separate
privacy notice describing the study's processing of personal data.

## Open and close intake

| Intake state | Use it for |
| --- | --- |
| Inactive | Keep the condition closed to new participants. |
| Testing | Run a pilot. Testing sessions appear in monitoring and are excluded from research corpus exports. |
| Active | Admit research participants under the selected condition. |

Open **Start experiment** and start **Local preview** for the pilot. Play both seats in separate browser profiles and check the resulting sessions in the dashboard. Use **Official intake** for data collection and distribute the URL from the running experiment. Record the game package revision, experiment ID, and recruitment link together.

Use **Pause intake** when recruitment ends. Keep the host running while participants finish. On restart, reopen intake explicitly. See [monitor sessions and export results](data-operations/) before archiving a study.
