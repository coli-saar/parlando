# Prolific integration

Parlando can accept participants from Prolific, correlate each game session with the corresponding
Prolific submission, and return participants through outcome-specific Prolific completion paths
when their outcomes have configured codes. The integration deliberately stops at that boundary:
Parlando does not call the Prolific API and therefore does not know whether a submission was
approved, returned, rejected, or paid.

This guide describes the implemented integration and its current operational limitations.

## Mental model

Three identifiers connect one Prolific visit to Parlando:

| URL parameter | Meaning |
| --- | --- |
| `PROLIFIC_PID` | The participant's Prolific identifier |
| `STUDY_ID` | The Prolific study identifier |
| `SESSION_ID` | The identifier for this participant's specific Prolific submission |

`SESSION_ID` does not identify a Parlando game session. Two Prolific participants who are paired in
one Parlando session have different Prolific `SESSION_ID` values. Parlando creates its own public
session identifier for the game and its own opaque participant credential for authentication.
Prolific identifiers are correlation data, never login or resume credentials.

Parlando then separates two kinds of outcome:

- the runtime records a provider-neutral participant outcome, such as `completed` or
  `partner_left`; and
- the experiment configuration maps that outcome to a Prolific **completion path** by its custom
  completion code.

The Parlando outcome names are not built-in Prolific path names. In Prolific, you create custom
completion paths, choose their participant-facing labels and submission-processing actions, and
copy their custom completion codes into the corresponding Parlando fields.

## Configure a study

Configure completion paths before publishing the Prolific study. Prolific warns that changing paths
after a study goes live can disrupt submissions already in progress.

### 1. Create the completion paths in Prolific

Open the study's **Data collection → Completion paths** section. Create one custom completion path
for each of the five standard Parlando outcomes below. Choose the label and processing action that
match the study's compensation and review policy.

| Parlando outcome | When this participant receives it |
| --- | --- |
| `completed` | The game reaches its normal completion condition |
| `partner_left` | The other participant leaves or does not reconnect in time |
| `partner_unavailable` | Matchmaking ends before a partner becomes available |
| `timed_out` | This participant's session reaches a runtime time limit |
| `technical_failure` | Parlando or supporting infrastructure prevents continuation |

Prolific completion paths can have automated submission-processing actions. Parlando does not
choose those actions and does not infer them from the outcome name. In particular, a path named
`partner_left` in Parlando does not by itself promise approval, a return request, or payment.

See Prolific's current documentation for
[custom completion codes](https://researcher-help.prolific.com/en/articles/445170-custom-completion-codes)
and [returning participants to Prolific](https://researcher-help.prolific.com/en/articles/445178-what-survey-experimental-software-is-compatible-with-prolific).

### 2. Configure the same paths in Parlando

In the Parlando dashboard, select the experiment and open **Configuration**. The **Prolific
recruitment and completion paths** section appears below the text-to-speech settings.

1. Enable Prolific intake.
2. Enter the exact Prolific study ID accepted by this experiment.
3. Enter the custom completion code from each matching Prolific completion path.
4. Save the configuration and resolve any activation issues before starting testing or research.

Every standard path needs a code while Prolific intake is enabled. A manual code may contain up to
64 ASCII letters or digits. The dashboard's generator produces a readable code made from exactly two
uppercase words, without digits or punctuation. Generated codes are convenient suggestions; the
same values must still be configured as custom completion codes in Prolific.

### 3. Configure the participant URL in Prolific

Use the selected experiment's participant-page path as the external study URL. In Prolific, select
the option to supply URL parameters and retain the default parameter names:

```text
PROLIFIC_PID
STUDY_ID
SESSION_ID
```

Prolific normally appends these values to the external URL. Although Prolific describes `STUDY_ID`
and `SESSION_ID` as optional parameters for some survey platforms, Parlando requires all three when
Prolific intake is enabled. It also requires `STUDY_ID` to match the value in the experiment
configuration exactly.

The dashboard's **Participant page** link is a local test link and already contains synthetic values.
Do not copy those dummy query values into a live Prolific study. Use the same experiment path without
the synthetic query string and let Prolific supply the real parameters.

## Participant intake

The browser reads the three Prolific parameters once, sends them in the participant-intake request,
and removes them from the visible address bar. The server validates their presence, the expected
study ID, and bounded ASCII input before creating a participant credential.

The opaque Parlando credential is stored in the current tab's `sessionStorage`. It supports a
same-tab page reload and cannot be reconstructed from `PROLIFIC_PID` or `SESSION_ID`. Reopening the
study in another tab or device is not equivalent to reconnecting the original participant.

A repeated `SESSION_ID` is not a supported authentication or resume mechanism. Researchers should
not reuse one synthetic dashboard test URL for multiple simulated participants, because it
represents one Prolific submission.

## Consent and voluntary departure

When an experiment has consent items, a Prolific participant sees **Do not consent** beside the
ordinary waiting-room entry button. Direct participants do not see this action. The current button
returns to the Prolific submissions page before Parlando registers the participant or records consent.
It does not emit a completion code.

This behavior is a known policy gap. Prolific's current
[no-consent guidance](https://researcher-help.prolific.com/en/articles/445165-can-i-screen-participants-within-my-study)
recommends a dedicated **No consent** completion path with **Request a return**. Parlando does not yet
configure a sixth no-consent code or redirect to such a path. Confirm that the implemented manual
return flow is acceptable for the study before launch; otherwise the integration must be extended
before recruiting participants.

After registration, **Leave game** is a durable session-ending action. The leaving participant gets
the provider-neutral outcome `withdrew`, while the partner gets `partner_left`. Parlando does not
issue a completion code for `withdrew`; the terminal screen tells a Prolific participant to return
the submission manually. The term `withdrew` here means that the participant left the Parlando
session. It does not automatically revoke a recorded consent declaration or delete data.

If a participant withdraws consent to data processing, use the dashboard's participant-data
deletion control and follow the study's approved withdrawal procedure. Prolific's guidance also
distinguishes returning a submission from deleting data held by the external study software.

## Terminal screens and completion codes

Every participant receives the same game-specific completion content, whether they arrived directly
or through Prolific. For a Prolific participant, the shared Parlando client appends a provider widget
that contains:

- the custom completion code selected for that participant's outcome;
- a **Copy** button with a manual-copy fallback; and
- a **Return to Prolific** link of the form
  `https://app.prolific.com/submissions/complete?cc=CODE`.

The runtime selects the code only after it has durably recorded the terminal session outcome. In a
two-participant session, the two participants may therefore receive different completion codes. For
example, the participant who leaves receives no code, while the remaining participant receives the
configured `partner_left` code.

Opening the completion URL supplies the code to Prolific. It does not prove to Parlando that the
submission was accepted or processed. The current implementation also does not persist whether the
participant clicked the return link or copied the code.

Game code must not read Prolific parameters, choose completion codes, or render provider-specific
screens. A game supplies normal completion content through `ParticipantApp`; the Rust runtime and JS
client own participant outcomes, Prolific handoff, explicit leave, reconnection, and terminal input
blocking. Generated games only need to style the shared lifecycle and handoff classes.

## Dashboard visibility

The Sessions view labels the recruitment source as **Prolific** when a session contains a Prolific
participant. It shows the Prolific participant, study, and submission identifiers beside the relevant
A/B participant only inside the protected dashboard.

If the private Prolific correlation rows are later purged, the session and its A/B assignments remain
visible. The non-identifying fact that recruitment used Prolific also remains, so the dashboard still
labels the session as Prolific. Only the three provider identifiers disappear.

## Privacy and deletion boundary

Parlando stores `PROLIFIC_PID`, `STUDY_ID`, and `SESSION_ID` in the dedicated
`prolific_submissions` table. These values are:

- persistent across server restarts;
- visible only through protected dashboard session details;
- absent from participant protocol messages and browser game state;
- excluded from full, research, corpus, and single-session exports; and
- omitted from ordinary session events and application diagnostics.

The durable participant row retains `identity_provider = prolific`, which is a provider
classification rather than a Prolific identifier. This classification is intentionally retained
after the private correlation row is removed so historical sessions remain intelligible.

The dashboard's participant-data deletion is a broader privacy operation that concerns the
participant's research-session graph, not only Prolific correlation. Independently purging only
`prolific_submissions` preserves the pseudonymous research data, but there is currently no
experiment-level dashboard control for that narrower purge. Its retention period and operational
deletion procedure must therefore be defined by the deploying institution. Deleting the live table
does not remove identifiers from pre-existing database or off-service backups; backup retention must
be handled separately.

## Test without a Prolific study

The dashboard provides a synthetic intake route so the full browser flow can be tested locally.

1. Enable Prolific and configure the study ID and all five codes.
2. Start the experiment in testing mode.
3. Open **Participant page**. The link contains dummy `PROLIFIC_PID`, the configured `STUDY_ID`, and
   a synthetic `SESSION_ID`.
4. Open a second participant page when testing a human–human game. Reload the dashboard first so the
   second link receives a different synthetic Prolific submission identity.
5. Complete or leave the game and verify the terminal explanation, selected code, copy button, and
   return link.
6. In the Sessions view, verify **Recruitment: Prolific**, the A/B assignment, the participant
   outcome, and the private Prolific identifiers.
7. Export the test session and recursively check that none of the three synthetic identifiers occurs
   in keys or values.

The automated runtime and client tests cover outcome routing, explicit leave, agent shutdown,
reconnection, export exclusion, deletion of private correlation rows, and terminal rendering. Manual
testing is still useful for the final Prolific study configuration because Parlando cannot inspect
the labels or processing actions configured on Prolific.

## Reconnection and reload behavior

The participant client sends a heartbeat every second. A lost connection pauses interaction and the
partner sees a countdown based on the server's configured reconnect grace period. A same-tab reload
can reclaim the participant role with the opaque tab credential. If the deadline expires, the
runtime records a terminal outcome and selects the appropriate Prolific handoff.

Recovery is intentionally bounded. It does not guarantee recovery across browser tabs, devices, or
a server restart. A participant who reconnects after the terminal transition cannot revive the
session.

## Troubleshooting

### “Prolific participant parameters are required”

The experiment has Prolific intake enabled, but the participant URL lacks one or more of
`PROLIFIC_PID`, `STUDY_ID`, and `SESSION_ID`. Use the dashboard's synthetic participant link for local
testing or enable URL parameters in the Prolific study.

### “This Prolific study id is not accepted”

The URL's `STUDY_ID` does not exactly match the accepted study ID in the selected Parlando
experiment. Check that the participant was sent to the intended experiment and that Prolific and
Parlando use the same study ID.

### The experiment is invalid after enabling Prolific

All five standard completion paths require alphanumeric codes. Configure the paths in Prolific,
copy the codes into Parlando, save a new configuration revision, and then start testing or research.
Historical sessions remain visible while the configuration is invalid.

### Prolific reports an unexpected or missing code

Compare the terminal code with both the Parlando outcome field and the matching Prolific completion
path. Parlando constructs the completion URL directly from the configured code; it cannot verify
that the same code exists in the Prolific study. Do not treat `NOCODE` or an unexpected code as proof
of participant misconduct without reviewing the session and Prolific's current guidance.

### The terminal screen has no Prolific instructions

Confirm that the participant entered through a URL carrying all three Prolific parameters. A
participant in a direct-intake experiment does not receive provider-specific handoff content. A
Prolific participant who deliberately leaves receives manual-return instructions but no completion
code.

## Pre-launch checklist

- The accepted Parlando study ID equals the live Prolific `STUDY_ID`.
- Prolific supplies all three URL parameters with their default names.
- All five Parlando codes exactly match their Prolific completion paths.
- Each Prolific path's label and processing action match the approved study policy.
- The no-consent behavior has been reviewed against current Prolific guidance.
- Normal completion, partner loss, manual leave, and copy/redirect behavior have been tested.
- The dashboard shows the expected recruitment source and private identifiers.
- Exports contain no Prolific identifiers.
- Participant-data deletion, Prolific-correlation retention, and backup retention are documented for
  We