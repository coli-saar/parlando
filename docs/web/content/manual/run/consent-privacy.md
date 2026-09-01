---
title: Prepare consent and privacy
description: Adapt participant information, configure declarations, inspect the generated processing record, and retain the approved evidence.
---

# Prepare consent and privacy

Privacy in a dialogue study follows the complete data flow: what participants are told, what they
declare, what the task and providers receive, what Parlando retains, and what later leaves the
installation. Parlando implements controls and records technical evidence. The responsible
institution must supply the legal basis, approved wording, provider agreements, retention policy,
access procedure, and release decision.

This chapter describes the configuration workflow, not legal advice. Complete it while the
experiment is inactive and before Prolific or official intake is enabled.

## Prepare the participant information

Start from the [participant-information template](../privacy/participant-information-v1.0/). Copy it
to a public document controlled by the institution, then replace every `{{PLACEHOLDER}}`. Remove
statements about agents, Speechmatics, ElevenLabs, Prolific, typed dialogue, or research reuse when
the study does not use them. Do not describe every capability Parlando could enable; describe the
condition participants will actually encounter.

The final document should identify the controller and contacts, task and expected duration,
voluntary nature of participation, possible software-agent partner, retained data, external
services, purpose, access, retention, withdrawal and deletion procedure, risks, and intended
research or corpus use.

Have the document reviewed under the institution's procedure. Give the approved text a local
version such as `great-tree-study-1.0` and publish it at a stable HTTPS URL. A material wording
change requires a new local version even if the Parlando template version has not changed.

## Configure information and declarations

Open the inactive experiment and select **Configuration → Participant access and consent**:

1. Enter the approved participant-information version.
2. Enter the public HTTPS URL and open it in a separate browser to verify that it resolves without
   administrator authentication.
3. Add the declaration templates that correspond to the approved document.
4. Open every added item, replace its placeholders, choose whether it is required, and compare its
   wording with the participant information.
5. Save a new configuration revision with a change summary that names the approved information
   version.

{{< figure src="manual/images/experiment-consent.png" alt="Participant access and consent section with information URL, version, and consent-template controls" caption="The experiment records a version and URL for participant information together with the exact declaration presentation." >}}

The [declaration template](../privacy/consent-items-v1.0.yaml) is a machine-readable starting point.
Its supplied items assume that consent is the relevant basis; adapt the information, declarations,
and withdrawal language when the institution relies on another basis. A checkbox does not make
unnecessary collection necessary or unclear information clear.

Parlando records the exact configured representation and each participant's decision. A declined or
missing required declaration prevents registration. Declining before registration creates no
Parlando participant record. Declaration evidence remains available for restricted administrative
review and is omitted from the corpus candidate.

## Verify the participant path

Start **Local preview** and open the participant URL in a new private browser context. Check the path
as a participant would encounter it:

1. The information link opens the approved version.
2. Every intended declaration appears once and uses the approved wording.
3. Declining a required item prevents entry and presents the expected terminal path.
4. Accepting all required items proceeds to device preparation when configured and then the waiting
   room.
5. Returning with the same browser follows the expected credential-recovery behavior.

Repeat this check after any change to participant information, declarations, recruitment, pairing,
or speech. The saved form can be structurally valid while the participant procedure is still
inconsistent or confusing.

## Minimize the data before collection

Every live session stores accepted actions, resulting task states, completion, and limited
diagnostic information. Task authors therefore minimize the core record by keeping `State`,
`Action`, `Completion`, transition metadata, and game logs no richer than the research purpose
requires.

Optional modalities add records. Typed messages are stored when players send them. Final transcripts
and utterance timing are stored when recognition produces them. Omit a typed-message control and
leave recognition disabled when language is not needed.

Parlando does not durably store raw microphone audio, synthesized audio, microphone device IDs or
labels, user-agent strings, or arbitrary browser error text. When transcription is enabled,
Speechmatics receives live microphone audio. When agent speech is enabled, ElevenLabs receives
agent-authored text and technical voice parameters. Provider-side logging and retention depend on
the institution's agreement and provider settings.

Infrastructure outside Parlando also belongs in the assessment. Hosting logs, backups, agent or
model services, recruitment systems, and analyst copies may retain identifiers or content that
Parlando itself does not retain.

## Review the generated processing record

After saving the experiment, open **Privacy**. Parlando derives a processing record from the exact
revision. It reports the participant arrangement, enabled speech services, retained categories,
verified non-retention, declaration configuration, deletion behavior, and corpus export boundary.

{{< figure src="manual/images/experiment-privacy.png" alt="Privacy tab showing downloadable data documentation and the generated data-processing record" caption="The generated record reports facts Parlando can establish from the saved revision. Download it after the configuration is final." >}}

Read the report rather than merely downloading it. A disabled service should appear disabled; an
enabled provider should appear as a recipient; participant information and declarations should show
the expected versions. Return to Configuration and save another revision when the report exposes a
mismatch.

Download both Markdown and JSON. Add the institutional facts Parlando cannot infer: controller,
legal basis, approval, contracts, retention schedule, access procedure, backup policy, and release
assessment. Retain the combined material with the study. The repository's
[German review material](../privacy/) supports an institutional assessment of a self-hosted
installation but is not an automatic approval.

## Plan linkage, deletion, and release

Parlando assigns readable pseudonyms within an experiment and does not ask for a participant name.
It does not create a platform-wide human identity across experiments. These properties reduce
linkage; they do not make dialogue anonymous. Recruitment identifiers, external logs, unusual task
behavior, and participant-authored language may still identify a person.

Keep the recruitment mapping separate from ordinary analysis and restrict it. Participant deletion
removes every shared session in which that participant appeared, including the counterpart's
contribution. The dashboard previews this scope and blocks deletion while a session is unfinished.
The retention procedure must also address backups, exports, analyst copies, and releases, because
deleting the live record does not alter those copies.

The corpus candidate omits recruitment identity, administrator data, credentials, declaration
evidence, and operational identifiers. It retains experiment-scoped labels and free
dialogue. Before release, remove remaining mappings, review messages and transcripts, inspect
game-specific state and completion, document transformations and exclusions, and obtain the
required approval.

Once the privacy record and participant path agree with the intended study, the next chapter can
link the same inactive experiment to Prolific. Studies without Prolific may proceed directly to the
data-operations and deployment chapters.
