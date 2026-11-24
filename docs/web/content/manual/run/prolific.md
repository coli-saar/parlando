---
title: Recruit through Prolific
---

# Recruit through Prolific

Use the game dashboard to configure Prolific credentials and verify the linked
study before opening intake.

## Configure the study

1. Run the host on a persistent HTTPS deployment. Set the public participant origin with `parlando serve --public-url` or with `Server::public_url` for a custom Rust host.
2. In **Game settings**, add the Prolific API token under **Provider secrets**.
3. Configure the condition while its intake is inactive. Under **Configuration → Prolific study and completion paths**, copy **URL for Prolific study setup** into the external study in Prolific.
4. Configure Prolific to record IDs through URL parameters and retain the placeholders in the copied URL. Enable **Secure external URL** if your Prolific workspace offers it.
5. Create the completion paths below and enter their codes and the exact study ID in Parlando. Copy the segment immediately after `/studies/` in the Prolific study URL. Organization URLs can include a workspace segment before `/studies/`. Save the experiment and resolve any preflight errors.

The public URL must be reachable by participants. Use an estimated completion time that covers the configured partner wait and a maximum allowed time that covers the Parlando session lifetime.

Use the built-in **No consent** path and choose **Custom** for outcomes without a
matching preset. Use the outcome names as labels to identify the paths in
Prolific. Parlando uses the exact code in each field to find the path and checks
that it has the required action.

| Outcome | Prolific action expected by Parlando |
| --- | --- |
| Game completed | Automatically approve |
| Partner left | Automatically approve |
| Game did not start | Request return |
| Participant left before completion | Request return |
| Technical failure | Automatically approve |
| No consent | Request return through the built-in No consent path |

## Test and open intake

Start with **Local preview** to inspect the task and consent screens. Use
**Test through Prolific** to verify real launch identity, pairing, and
completion handoff. Test with two participants and check at least one
non-completion path. Open **Official intake** before the published study
starts sending participants.

Monitor submissions in Prolific alongside sessions in Parlando. The
Parlando outcome records what happened in the task. Prolific controls
submission processing and payments.

## Find a participant's session

Open the experiment's **Sessions** tab, enter the last few characters of the
Prolific participant ID, and select **Search**. Parlando searches sessions in
the selected experiment and applies the lifecycle filter to the results.
The search uses retained Prolific participant IDs. Enter a longer suffix if
several participants match. Select **Clear** to return to the recent sessions list.
