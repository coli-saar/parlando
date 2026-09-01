---
title: Run a local pilot
description: Create a Great Tree experiment, play one two-browser session, inspect it, and download the result.
---

# Run a local pilot

This procedure turns the running Great Tree example into one complete local data-collection cycle.
You will create an experiment, enter from two isolated browsers, finish a session, inspect the
record, download an export, and close intake. It takes about fifteen minutes after the server is
running.

## Before you begin

Complete [Install and run Great Tree](getting-started/). Keep the server terminal open and verify
that [http://127.0.0.1:8080/admin](http://127.0.0.1:8080/admin) shows the dashboard. You need two
isolated browser contexts—for example, one normal window and one private window—so that Parlando can
assign two distinct participant credentials.

For this pilot, do not configure Prolific, speech providers, or a software agent.

## 1. Create the experiment

1. Open `/admin` and select **New experiment** in the lower-left corner.
2. Enter `great-tree-pilot` as the experiment ID. The ID becomes part of the participant URL and
   cannot be renamed later.
3. Optionally add a private note such as `Local two-browser verification`.
4. Select **Create experiment**.

The new experiment should appear in the left catalogue with status **Inactive** and configuration
revision 1. Keep it inactive while editing.

## 2. Configure a minimal condition

Open **Configuration**. Use these settings:

| Section | Setting for this pilot |
| --- | --- |
| Session lifecycle | Waiting 600 seconds; reconnect 15 seconds; idle 1800 seconds; maximum lifetime 14400 seconds |
| Participant access and consent | Direct access enabled; no production consent template for this technical pilot |
| Voice, recognition, and text-to-speech | Disabled |
| Prolific | Disabled |
| Players and agents | Human vs human |
| Game configuration | `{}` |

Select **Validate YAML** beside the game options. Then enter `Minimal local pilot` in **Optional
change summary** and select **Save new revision**. A successful save increases the revision number
and adds an entry to **Revision history**.

{{< figure src="manual/images/experiment-configuration.png" alt="Great Tree experiment configuration showing lifecycle fields for an inactive local pilot" caption="Configuration is edited only while the experiment is inactive. Saving creates a new numbered revision rather than changing earlier revisions." >}}

If saving fails, use the validation message beside the affected field. Do not enable unrelated
features to clear a warning: provider settings, consent, and agents add requirements of their own.

## 3. Start Local preview

1. Open **Start experiment** at the upper right.
2. In the launch menu, find **Local preview** and select its **Start** button.
3. Select **Open** to open the participant page, or copy the URL. It has the form:

   ```text
   http://127.0.0.1:8080/e/great-tree-pilot/
   ```

Local Preview labels the resulting session as testing data. Testing sessions remain visible in the
dashboard but do not enter the corpus candidate.

## 4. Play from two browser contexts

1. Open the participant URL in the first browser context and select **Enter waiting room**.
2. Open the same URL in a private or otherwise isolated context and enter there as well.
3. Wait for Parlando to pair the two participants and assign seats `A` and `B`.
4. In the Crown view, change at least one sunlight gate. In the Root view, change at least one water
   gate. Continue until three limbs flower.

You have exercised both role-specific observations when one browser shows branches and the other
shows roots. You have completed the task when both browsers show the shared success result.

{{< figure src="manual/images/great-tree-startup.png" alt="Great Tree participant entry screen with an Enter waiting room button" caption="Open this page twice in isolated browser contexts. The standard participant flow creates the credentials and waiting-room entry for the game." >}}

{{< figure src="manual/images/great-tree-game.png" alt="The Great Tree Crown participant view with five branch controls" caption="The game-specific screen renders only the current role's observation. A second browser receives the Root view." >}}

## 5. Inspect the recorded session

Return to `/admin`, open **Sessions**, and select the new session. Check that the detail view shows:

- two human participants assigned to different seats;
- the configuration revision and game version;
- accepted actions from both roles; and
- a completed lifecycle state with structured completion data.

Enable **Show setup events** only when diagnosing participant entry or readiness. Enable **Show
game/agent logs** only when the game or an agent wrote diagnostic events. Use **Operations** while a
session is live to inspect connection health, deadlines, and available capacity.

## 6. Download and inspect an export

Open **Export**, keep **Corpus candidate**, choose JSON, and select **Download export**.

{{< figure src="manual/images/experiment-export.png" alt="Experiment Export tab with corpus candidate and JSON selected" caption="The Export tab produces the public corpus candidate in JSON, YAML, or CSV. Local Preview sessions are deliberately excluded." >}}

Because this was a Local Preview run, the corpus candidate should contain no session from this test.
That is the expected result. To verify the export with disposable non-testing data, stop the preview,
start **Official intake**, repeat the two-browser session, and download again. Do this only with
deliberately fictional test input, because Official intake creates an ordinary research session.

## 7. Stop intake

Select **Stop** when the pilot is finished. New participants can no longer enter; established
sessions may finish. After a server restart every experiment returns to **Inactive**, so reopening
intake is always deliberate.

You now have a verified local participant path and have seen how a compiled game becomes a recorded
experiment. The next part builds that compiled game: continue with
[Build a dialogue game](../build/create-game/). Later chapters return to the dashboard to prepare a
real condition, consent, recruitment, deployment, and data handling.
