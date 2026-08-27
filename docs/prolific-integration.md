# Prolific integration

Parlando verifies each Prolific admission, pairs Prolific participants without changing their
recruitment source, records phase-specific outcomes, and returns each participant through the
appropriate Prolific completion path. Parlando never calculates compensation or sends bonus
payments. Researchers review factual session records in Parlando and perform any payment-related
work in Prolific.

## Mental model

One dyadic Parlando game contains two Prolific submissions. Each participant therefore has their
own `PROLIFIC_PID` and `SESSION_ID`; both share the configured `STUDY_ID`. The Parlando game has a
separate session identifier and assigns the two verified participants roles A and B.

Parlando records ten factual participant outcomes and maps them onto five researcher-configured
Prolific completion paths:

| Parlando outcome | Meaning | Prolific path | Required Prolific action |
|---|---|---|---|
| `completed` | The game completed normally. | Completed | Automatically approve |
| `left_waiting_room` | The participant actively left before a partner was found. | Unmatched | Request return |
| `left_game` | The participant actively left after the game began. | Timed out | Request return |
| `participant_inactive` | The participant failed a required participation check. | Timed out | Request return |
| `connection_lost` | This participant disconnected and did not reconnect in time. | Timed out | Request return |
| `partner_left` | The participant remained while the partner left or failed to reconnect. | Partner left | Automatically approve |
| `partner_unavailable` | No partner was found before the waiting deadline. | Unmatched | Request return |
| `idle_limit_reached` | The running game produced no accepted message or action before its idle deadline. | Timed out | Request return |
| `technical_failure` | Parlando could not continue the session safely. | Technical failure | Automatically approve |
| `lifetime_limit_reached` | The absolute infrastructure lifetime ended an otherwise live game. | Technical failure | Automatically approve |

The path names are explanatory. The values configured in Parlando are the exact custom completion
codes created in Prolific. All five codes must be present and distinct.

## Configure the installation

An administrator configures three installation-level values in the Parlando dashboard:

1. Store a Prolific API token under **Provider secrets**.
2. Enter the Prolific workspace ID under **Shared settings**.
3. Keep **Prolific API base URL** at `https://api.prolific.com` for real studies. A compatible
   loopback emulator may use a local HTTP origin during integration testing.

Parlando verifies the workspace through Prolific and displays its title. The token is stored as a
protected server-side secret and is never written into experiment configuration or returned to the
browser.

## Configure the Prolific study

Create five custom completion codes in Prolific before activating the Parlando experiment:

1. Completed — **Automatically approve**.
2. Partner left — **Automatically approve**.
3. Unmatched — **Request return**.
4. Timed out — **Request return**.
5. Technical failure — **Automatically approve**.

The Unmatched path must be an ordinary custom completion code with **Request return**. Do not use a
screen-out action. A returned unmatched submission is visible in Prolific, where the researcher can
review the participant and make any manual payment that the study requires. Parlando does not create
a payment queue, calculate a waiting amount, or call a bonus endpoint.

Activate the Parlando experiment, then copy its **Participant page** link into Prolific as the
external study URL. The active link contains Prolific's literal `{{%PROLIFIC_PID%}}`,
`{{%STUDY_ID%}}`, and `{{%SESSION_ID%}}` placeholders; Prolific replaces them for each launch.
Enable Prolific's **Secure external URL** for the study. Prolific then adds a short-lived signed `prolific_token` query
parameter. Parlando verifies its signature, issuer, audience, expiry, study, workspace, participant,
and submission before admitting the participant. The browser removes this token and the provider
identifiers from the visible URL after intake.

See Prolific's current [study API and Secure external URL contract](https://docs.prolific.com/api-reference/studies/create-study)
and [JWKS endpoint guidance](https://docs.prolific.com/api-reference/well-known-endpoints/get-study-jwks).

If Secure external URL is unavailable for a particular study, Parlando performs the narrower
fallback verification by retrieving the claimed submission from the Prolific API and checking that
its participant and study match. The fallback is not an unchecked query-parameter mode.

## Configure the Parlando experiment

In **Configuration → Prolific**:

1. Enable Prolific intake.
2. Enter the exact Prolific study ID.
3. Copy the five codes from Prolific into the five corresponding fields.

Parlando does not generate codes. The experiment is invalid until every required field is present
and the codes are distinct.

When the researcher activates the experiment, Parlando retrieves the study from Prolific and checks:

- the study belongs to the configured workspace;
- its external URL targets this Parlando experiment;
- the five configured codes exist with exactly the required actions;
- the Unmatched code is not a screen-out path;
- the Prolific estimated completion time is compatible with the Parlando game limit; and
- the Prolific maximum time is compatible with Parlando's maximum session lifetime.

Activation fails with a specific configuration issue if a check cannot be completed. This prevents
participant intake from opening with a mismatched live study.

## What participants see

### Waiting for a partner

The waiting-room widget shows the server-owned deadline as a live countdown. It explains that the
participant may leave immediately or wait until the deadline, and that either route produces the
same Unmatched/return handling; the dashboard records the longer wait when the deadline expires.

The waiting duration is not estimated from browser heartbeats. Parlando stores the server timestamps
`waiting_started_at` and `ended_at` (or the fixed `waiting_deadline_at`) and displays their difference.
Prolific does not learn those minutes automatically from Parlando. A researcher who needs them for a
manual payment reads them in the Parlando session detail and performs the payment in Prolific.

### Running game

The game begins as soon as both required participants are ready. Three independent safeguards apply:

- **Reconnect deadline:** if one participant's game connection disappears, interaction pauses for a
  fixed grace period. A successful reconnect resumes the same session. At expiry, the disconnected
  participant receives `connection_lost`; the participant who remained receives `partner_left`.
- **Idle deadline:** only an accepted conversation message or game action advances meaningful
  activity. Heartbeats, connection traffic, and merely leaving a tab open do not count. If no
  meaningful activity occurs before the deadline, each participant receives `idle_limit_reached`,
  which maps to the request-return Timed out path.
- **Maximum lifetime:** an absolute infrastructure limit prevents a session from running forever.
  Reaching it is treated as a technical failure rather than participant inactivity.

If the Parlando process restarts, any session that was still waiting or running is finalized once as
a technical failure before new intake opens. Its original deadlines and participant assignments are
retained, and returning participants receive the durable Technical failure handoff. Parlando does
not attempt to reconstruct an interrupted real-time game from browser state.

### Terminal handoff

The terminal widget explains the participant's factual outcome and provides the appropriate Prolific
completion link. Each member of a pair receives the result derived for that participant; one partner
leaving never changes the other participant's recruitment source or reuses the leaver's return code.

## Review sessions

The dashboard session list labels unmatched and partner-left sessions explicitly. Session detail
shows:

- waiting start, waiting deadline, and exact unsuccessful waiting duration;
- both A/B assignments and both participants' Prolific identifiers;
- recipient-specific factual outcomes and completion handoffs;
- which participant disconnected or left;
- the reconnect deadline and whether the partner remained; and
- the last meaningful activity time for idle-limit cases.

A participant who reached Parlando's waiting room remains visible even when no match was formed.
Private Prolific correlation can be purged under the configured retention policy without deleting the
non-identifying session outcome or its A/B research record.

## Operational boundary

Parlando may read a submission's current Prolific status, entered code, and return-request state for
reconciliation. It does not approve, reject, return, or pay submissions through the API in this
version. Prolific remains the authority for submission processing and all compensation.

Before opening a paid study, run a pilot with two real submissions and verify activation preflight,
pairing, normal completion, unmatched timeout, partner disconnect, terminal links, and dashboard
facts from both participants' perspectives.

Use [Test the Prolific integration](prolific-testing.md) to run Parlando's local process-boundary
scenario matrix before the live pilot.
