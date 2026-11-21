---
title: Recruit through Prolific
---

# Recruit through Prolific

Use a single-game installation started with `parlando serve` for this workflow.
It has the game settings and Prolific preflight controls used below. The shared
multi-game host has no dashboard for provisioning Prolific credentials.

## Configure the study

1. Run the built package on a persistent HTTPS deployment. Pass its participant origin as `--public-url` to `parlando serve`.
2. In **Game settings**, add the Prolific API token under **Provider secrets**.
3. Configure the condition while its intake is inactive. Under **Configuration → Prolific study and completion paths**, copy **URL for Prolific study setup** into the external study in Prolific.
4. Configure Prolific to record IDs through URL parameters and retain the placeholders in the copied URL. Enable **Secure external URL** if your Prolific workspace offers it.
5. Create the completion paths below and enter their codes and the exact study ID in Parlando. Save the experiment and resolve any preflight errors.

The public URL must be reachable by participants. Use an estimated completion time that covers the configured partner wait and a maximum allowed time that covers the Parlando session lifetime.

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
