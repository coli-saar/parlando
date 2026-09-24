---
title: Recruit through Prolific
---

# Recruit through Prolific

Recruitment integration connects two records that serve different purposes. Prolific owns the study
listing, recruitment identity, submission, and payment state. Parlando owns admission to a specific
experiment, pairing, task events, and participant-specific outcome. Linking them should preserve
that separation rather than make either system an incomplete copy of the other.

One Prolific study should correspond to one Parlando experiment. A dyadic Parlando session then
contains two independently verified Prolific submissions that share a study ID but have different
participant and submission IDs.

{{< figure src="manual/images/prolific-completion-state-paths.svg" alt="Participant outcomes and their Prolific completion paths" caption="Parlando records a provider-neutral outcome first, then maps that outcome onto the completion path configured for the experiment." >}}

## Understand the trust boundary

An arrival URL contains `PROLIFIC_PID`, `STUDY_ID`, and `SESSION_ID`, but Parlando does not trust
those values by themselves. Enable Prolific's **Secure external URL** when available. Otherwise,
Parlando checks the claimed submission through the Prolific API. There is no unchecked mode.

The API token belongs to the installation and is stored as a server-side secret under **Game
settings**. The linked study and its completion codes belong to one experiment revision. This
separation lets one installation run several studies without copying a broad API credential into
each condition or participant browser.

## Model outcomes before configuring codes

Parlando records what happened to each participant. A Prolific completion path tells the provider
what submission action should follow. The factual outcome remains in Parlando even when several
outcomes use the same provider action.

Configure six paths:

| Parlando path | Required Prolific action | Rationale |
| --- | --- | --- |
| Completed | Automatically approve | The participant completed the task. |
| Partner left | Automatically approve | This participant remained available while the counterpart ended the session. |
| Game did not start | Request return | No playable task began. |
| Participation ended before completion | Request return | This participant left or became unavailable after start. |
| Technical failure | Automatically approve | Infrastructure prevented safe continuation. |
| Built-in No consent | Request return | Registration ended before a Parlando participant was created. |

**Game did not start** is an ordinary custom return path, not a screen-out path. Failure to form a
playable pair is a study outcome rather than an eligibility judgment.

Parlando does not approve, reject, return, or pay submissions through the API. It redirects the
participant to the configured path and records the relevant facts. Researchers review submission
state and apply waiting compensation or other payment policy in Prolific.

## 1. Prepare the installation and condition

You need a publicly reachable HTTPS deployment, an inactive Parlando experiment, a Prolific
researcher account, a draft Prolific study, and an API token with access to that study.

In **Game settings**, keep the API origin at `https://api.prolific.com`, enter the token, and save.
Treat the token as a password. It is not written into the experiment configuration or sent to a
participant.

Finish the experiment's ordinary participant information, declarations, pairing, communication,
privacy, partner-wait limit, and maximum lifetime before linking the study. Prolific's timing values
must describe the same participant procedure.

{{< callout type="warning" title="Use the public origin" >}}
A Prolific participant cannot reach a `localhost` URL on the researcher's computer. Configure the
server's public HTTPS origin before copying the study URL. The dashboard may itself use a separate
authorized administrator address because the server supplies the setup URL.
{{< /callout >}}

## 2. Link the external study

The inactive experiment displays **URL for Prolific study setup**. It has this structure:

```text
https://games.example.edu/e/condition-a/?PROLIFIC_PID={{%PROLIFIC_PID%}}&STUDY_ID={{%STUDY_ID%}}&SESSION_ID={{%SESSION_ID%}}
```

Paste the complete URL into the draft Prolific study and keep the three placeholders literal.
Prolific replaces them for each launch. Configure URL-parameter identity recording and enable
**Secure external URL** when available.

{{< figure src="manual/images/experiment-prolific.png" alt="Prolific study and completion paths section showing the setup URL and Enable Prolific intake control" caption="Copy the setup URL from the public deployment. Enabling intake reveals the linked-study and six completion-code fields farther down the same section." >}}

Set Prolific's estimated duration to at least the maximum time a participant may wait for a partner.
Set its maximum allowed time to at least Parlando's maximum session lifetime. These comparisons are
conservative: they ensure that the provider does not expire an ordinary participant while Parlando
still considers the procedure live.

Create the six completion paths in Prolific and copy their distinct generated codes. Then return to
Parlando:

1. enable Prolific intake;
2. enter the study ID rather than the complete study-page URL;
3. enter each code under its matching outcome; and
4. save the experiment configuration.

Saving checks API access, project and workspace, exact equality with the server-generated external
URL template, completion actions, and timing compatibility. A locally valid revision remains saved
when a provider-readiness check fails, so the exact draft can be corrected. Provider-backed intake
remains unavailable until the check succeeds.

The link is ready when the revision saves without a provider-readiness error and the launch menu
offers **Test through Prolific**. Do not publish the provider study yet.

## 3. Test the cross-system procedure

A Prolific-enabled experiment offers three run modes:

| Mode | Evidence it provides |
| --- | --- |
| Local Preview | Participant procedure and task behavior without contacting Prolific |
| Test through Prolific | Signed or API-verified launch, identity mapping, pairing, and redirects |
| Official intake | Production admission for the linked study |

Begin with Local Preview, but do not stop there: it cannot establish that provider admission and
completion handoff work. Test through Prolific with two participants because a dyadic condition
cannot exercise pairing with one. Verify normal completion and at least one non-complete path. If
the workspace has no test-participant facility, use a deliberately small pilot.

Each Local Preview **Open** or **Copy URL** action creates a fresh synthetic invitation. During Test
through Prolific and Official intake, launch participants from Prolific; Parlando does not open its
literal provider template as though it were a participant link.

For production, start **Official intake** in Parlando before publishing or scheduling the Prolific
study. Otherwise a valid provider launch may reach an experiment that is not admitting participants.
Monitor the first pair in both systems.

Pause intake to stop new admissions while established sessions finish. Mark the experiment complete
only when collection should not reopen. A server restart returns the experiment to inactive, so
reactivation after deployment is deliberate.

## 4. Reconcile facts and payments

Use Parlando to inspect verified recruitment source, pairing, waiting duration, departure cause,
participant-specific outcome, and returned completion path. Use Prolific to inspect submission and
payment state. When the two disagree, first determine whether the disagreement concerns a task fact
or a provider action; they are not interchangeable fields.

Common failures follow the same boundary:

- an invalid external URL differs from the server-generated template in its public origin,
  experiment path, parameter order, or literal placeholders;
- an inaccessible study usually has the wrong final study ID or an API token without project access;
- a completion conflict concerns the generated code or action, not merely its display name;
- a timing failure means the two systems describe incompatible participant durations; and
- a rejected arrival means intake is closed, identity evidence is missing, or the launch belongs to
  another study.

For current provider controls, consult Prolific's documentation for
[API tokens](https://docs.prolific.com/documentation/get-started/api-fundamentals),
[URL parameters](https://docs.prolific.com/docs/how-do-i-send-prolific-ids-to-my-study-via-url-parameters),
and [Secure external URLs](https://docs.prolific.com/docs/secure-external-url-parameters).

Once recruitment begins, the next chapter explains how to inspect live sessions and preserve the
result: [Monitor, export, and back up data](data-operations/).
