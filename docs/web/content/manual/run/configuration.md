---
title: Configure an experiment
description: Create an inactive condition, fill its participant, lifecycle, pairing, communication, capacity, and game settings, then save and review it.
---

# Configure an experiment

An experiment is a claim that a set of sessions share one interpretable condition. Parlando makes
that claim inspectable by storing the condition as an immutable numbered revision and attaching the
relevant revision to every session. Configuration is therefore part of the research record, not
merely a collection of runtime preferences.

## Configure one condition in the dashboard

Before you begin, start the compiled game, sign in at `/admin`, and create an experiment. Its status
must be **Inactive**; running experiments cannot be edited.

1. Open the experiment and select **Configuration**.
2. Under **Session lifecycle**, enter the waiting, reconnect, idle, and maximum-lifetime limits in
   seconds. Start with values exercised by a pilot rather than production guesses.
3. Under **Participant access and consent**, enable direct access if participants may use the
   experiment URL. Enter the approved participant-information URL and version, then add the required
   declaration items.
4. Under **Players and agents**, choose **Human vs human** or **Human vs agent**. If you select an
   agent, choose its implementation and fill the settings that identify the policy condition.
5. For speech, apply the complete human–human or human–agent configuration described in
   [Typed and spoken dialogue](../build/voice/). Enable Prolific only when recruitment uses it.
   Each integration creates additional required fields and readiness checks.
6. Under **Capacity**, set limits no higher than the deployment and external services sustained in
   testing.
7. Enter the game's YAML under **Game configuration** and select **Validate YAML**.
8. Add an **Optional change summary** that states the reason for this revision, then select **Save
   new revision**.

{{< figure src="manual/images/experiment-configuration.png" alt="Inactive Great Tree experiment on the Configuration tab with session lifecycle controls" caption="All condition settings live on the Configuration tab. The page continues through participant access, speech, recruitment, agents, capacity, and game configuration." >}}

A successful save changes the revision number and adds a timestamped entry to **Revision history**.
Open **Privacy** immediately afterward and confirm that its description of participants, speech
services, retained data, and export matches the condition you intended. Start Local Preview only
after both checks pass.

## Draw the boundary around the condition

Three levels of choice meet in a running study:

| Level | Question it answers | Examples |
| --- | --- | --- |
| Game | What task is being performed? | Rules, actions, observations, completion |
| Installation | Where and under whose control does it run? | Hosting, persistent data, administrator access, provider secrets |
| Experiment | Under what condition is this sample collected? | Recruitment, pairing, limits, communication, agents, data flow |

Only values required before the dashboard opens belong to server startup: network address,
persistent-data location, and compiled participant application. Shared institutional settings and
provider credentials belong to authenticated **Game settings**. Everything that may distinguish one
sample from another belongs to the experiment.

This boundary matters for comparison. If two samples differ in agent policy, voice provider, wait
limit, participant instructions, or enabled voice and speech modalities, they do not share the same
effective condition merely because the game binary is identical.

## Give the condition a durable identity

Choose an experiment ID that can remain stable in participant URLs, scripts, exports, and study
records. Use separate experiments for conditions you intend to compare. Revisions preserve edits,
but a single ID whose meaning changes halfway through recruitment is harder to reason about than two
explicit condition IDs.

Keep the experiment inactive while editing. A successful save validates the configuration and
creates a new revision rather than rewriting the previous one. If another administrator saves first,
reload the page and reapply your change to the current revision.

An experiment is tied to the semantic version of the compiled game under which it was created. If a
new binary changes that version, the earlier experiment remains inspectable and exportable but
cannot reopen. Clone it to create a condition validated against the new game while preserving the
old sessions' provenance.

## Design the participant path as one sequence

Participant entry combines information, declarations, recruitment identity, pairing, and technical
preparation. Read it as one procedure rather than five independent panels. A participant should
learn what will happen before registration, make any required declarations, prepare required
devices, wait under a stated policy, and enter the task with a defined counterpart.

Direct recruitment is sufficient for local work, institution-managed invitations, and studies that
do not need a recruitment-provider identity. Provider-backed recruitment adds admission
verification and outcome handoff; it does not replace the experiment's information, declarations,
or lifecycle policy.

Live pairing is either human–human or human–agent. In a human–agent session the human occupies seat
`A` and the agent seat `B`; game configuration may map those seats to different domain roles. Select
the agent implementation, policy settings, response timeout, invalid-action limit, and optional
seed as part of the condition. Agent-only batches use a separate
[headless run specification](../build/agent-agent/), not this live pairing control.

## Express the temporal contract

Four limits describe different obligations:

| Limit | What it guarantees will end |
| --- | --- |
| Waiting-session timeout | An attempt that does not obtain a playable counterpart |
| Reconnect grace | A temporary loss of one participant's connection |
| Session idle timeout | A connected game with no accepted action or message |
| Maximum session lifetime | Any unfinished session, regardless of activity |

Choose these values from the participant procedure and compensation policy. A waiting limit should
be plausible at the expected recruitment rate. An idle limit should allow reading and planning. The
maximum lifetime should exceed intended task duration while remaining finite.

Only an accepted action or message resets the idle timer. Leaving a connected browser tab open does
not keep an otherwise abandoned session alive indefinitely.

## Budget scarce resources

Capacity limits apply to research sessions, waiting sessions, unattached participant credentials,
and reserved transcription streams. Begin with a pilot-sized limit and raise it only after observing
CPU, memory, remote-agent latency, provider capacity, and database growth under the intended
modalities.

Persistent storage also needs headroom. A configured disk reserve can stop new admissions before
storage is exhausted; it cannot replace host-level monitoring or backups.

## Choose communication with its evidence consequences

Communication choices change both participant behavior and the data path. Parlando documents a
game-authored typed interface, transcribed human–human speech, and transcribed human–agent speech
with synthesized agent replies. [Typed and spoken dialogue](../build/voice/) explains how to build
and configure them. Typed-message controls belong to the game interface; speech settings belong to
the experiment. Do not infer additional study conditions from the individual configuration fields.

Parlando does not expose separate switches for storing each research-data category. Live sessions
store their accepted actions, resulting authoritative states, structured completion, and bounded
operational record. Typed messages are stored when typed communication produces them; final
transcripts are stored when transcription produces them. Parlando does not store raw microphone
audio.

Data minimization therefore happens mainly in task design and modality choice. Keep observations,
actions, completion records, and extension-defined logs no richer than the research purpose
requires. Omit a typed-message interface when the study does not collect typed dialogue, and use a
typed interface instead of speech when live audio and external speech processing do not serve the
research purpose. Before activation, inspect the experiment's generated privacy report for the
effective record rather than assuming that an unused field is a storage control.

## Validate the procedure, not only the form

A saved revision proves that types, ranges, required combinations, and available secrets are valid.
It does not prove that a participant can understand the information, obtain microphone permission,
reach a remote agent, find a partner, or return successfully to a recruitment provider.

Use Local Preview for the ordinary path, then exercise the deployed public origin. Test normal
completion, voluntary departure, unavailable partner, reconnect, idle expiry, maximum lifetime,
agent failure, and every enabled provider. Retain the final configuration revision and privacy
report with the study materials before opening official intake.

Before that intake opens, prepare the information and declarations that describe this exact
revision in [Prepare consent and privacy](consent-privacy/).
