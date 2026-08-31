# Run a Parlando experiment through Prolific

This guide takes an experimenter from an inactive Parlando experiment to a tested Prolific study.
You configure one Prolific study for one Parlando experiment. Prolific recruits the participants and
tracks their submissions; Parlando verifies their identities, pairs them, runs the game, and sends
each participant to the appropriate Prolific completion path.

## Before you begin

You need:

- administrator access to the Parlando dashboard;
- a deployed Parlando game that participants can reach through a public HTTPS URL;
- a Prolific researcher account with permission to create studies and projects;
- an inactive Parlando experiment; and
- a Prolific API token for the researcher account.

Do not use a URL beginning with `http://localhost` in a Prolific study. `localhost` refers to the
participant's own computer, so Prolific participants cannot reach your Parlando server. Open the
dashboard through its public address before copying a Prolific setup URL.

The API token is shared by the Parlando game installation. The linked study and its completion codes
belong to one experiment. You do not enter a Prolific workspace ID in Parlando: Parlando obtains the
workspace from the linked study's project.

## 1. Connect Parlando to the Prolific API

Create a researcher API token in Prolific before configuring the experiment:

1. In Prolific, open **API Tokens** from the researcher toolbar.
2. Select **Create API Token**.
3. Give the token a name that identifies the Parlando installation.
4. Copy the token. Treat it as a password; Prolific tokens do not expire automatically and inherit
   the researcher's permissions.

Then store the token in Parlando:

1. Open **Game settings** in the Parlando dashboard.
2. Find the **Prolific** section.
3. Keep **API base URL** set to `https://api.prolific.com`.
4. Enter the token in **API token**.
5. Select **Save all game settings**.

Parlando stores the token as a server-side secret. It does not place the token in experiment
configuration or send it to participants' browsers.

See Prolific's [API token guidance](https://docs.prolific.com/documentation/get-started/api-fundamentals)
for token creation, scope, rotation, and deletion.

## 2. Prepare the Parlando experiment

Create or select the experiment in Parlando and keep it inactive while editing it. Complete the
ordinary game, session, consent, and privacy settings before linking Prolific. In particular, decide
the maximum partner-wait duration and maximum session lifetime; Prolific's study timings must be long
enough to accommodate both values.

Open the experiment's **Configuration** tab and find **Prolific study and completion paths**. The
section shows **URL for Prolific study setup** even though the experiment is not running. Copy that
URL. It should resemble:

```text
https://games.example.edu/e/my-experiment/?PROLIFIC_PID={{%PROLIFIC_PID%}}&STUDY_ID={{%STUDY_ID%}}&SESSION_ID={{%SESSION_ID%}}
```

Keep the three placeholders exactly as shown. Prolific replaces them with a participant ID, study
ID, and submission ID whenever it launches a participant. Do not replace them yourself and do not
copy a Local Preview URL with fake identifiers.

## 3. Create the Prolific study

Create a new study inside the Prolific project that should own the experiment:

1. In Prolific, create a draft study and select the appropriate project.
2. In **What's the URL of your study?**, paste **URL for Prolific study setup** from Parlando.
3. Configure the study to record Prolific IDs through URL parameters.
4. Enter the study description, eligibility criteria, number of places, reward, and estimated
   completion time required by your study design.
5. Set the maximum allowed time, if you override Prolific's default, to at least the maximum Parlando
   session lifetime.
6. If the workspace offers **Secure external URL**, enable it. Parlando will verify Prolific's signed
   launch token. If the option is unavailable, Parlando verifies the submission through the Prolific
   API instead.
7. Keep the study as a draft until you have linked and tested it in Parlando.

If Prolific reports that the URL is invalid, first check its origin. The URL must use the public
deployment rather than `localhost`. The placeholder spelling above is Prolific's documented syntax.

Prolific documents these fields under
[the study object](https://docs.prolific.com/api-reference/studies/the-study-object).

## 4. Create six completion paths

In the draft Prolific study, open **Data collection → Completion paths**. Create the following six
paths and select the specified action for each one:

| Path in Parlando | Suggested name in Prolific | Prolific action |
|---|---|---|
| Completed | Completed | Automatically approve |
| Partner left | Partner left | Automatically approve |
| Game did not start | Game did not start | Request return |
| Participation ended before completion | Participation ended before completion | Request return |
| Technical failure | Technical failure | Automatically approve |
| No consent | Prolific's built-in No consent | Request return |

Prolific generates a code for each path. Keep the six codes available for the next step. Each code
must be distinct.

The **Game did not start** path must be an ordinary custom completion path whose action is **Request
return**. Do not configure it as **Screened out**. Failure to form a playable game is a study outcome,
not a failed eligibility screen.

Parlando does not calculate waiting compensation, issue bonuses, or change submission payment state
through the Prolific API. Review returned submissions in Prolific and apply the study's compensation
policy there.

## 5. Link the study to the Parlando experiment

Return to **Configuration → Prolific study and completion paths** in Parlando:

1. Turn on **Enable Prolific intake**.
2. Enter **Linked Prolific study ID**. If the Prolific study page has a URL such as
   `https://app.prolific.com/researcher/studies/abc123`, enter only `abc123`.
3. Copy each Prolific-generated completion code into its matching Parlando field.
4. Optionally describe the change in the configuration revision summary.
5. Save the experiment configuration.

Saving performs a live check against Prolific. Parlando verifies that:

- the API token can retrieve the study and its project;
- the study belongs to a project with a workspace;
- the study records participant identities through URL parameters;
- its external URL points to this Parlando experiment and includes all three placeholders;
- all six codes exist and have the actions shown above;
- the estimated duration is not shorter than the maximum partner wait; and
- the maximum allowed time is not shorter than Parlando's maximum session lifetime.

Parlando keeps the new revision when the local configuration is structurally valid, even if the
live Prolific check reports a provider-readiness issue. Read the condition on the Prolific run badge
and change the corresponding field in Prolific or Parlando. The experiment is not ready for a
Prolific-backed run until this check succeeds.

## 6. Choose a test mode

Open **Start experiment** while the experiment is inactive. A Prolific-enabled experiment offers
three modes:

| Mode | Use it for | Participant URL |
|---|---|---|
| Local Preview | Checking the game without contacting Prolific | Fake Prolific participant and submission IDs |
| Test through Prolific | An end-to-end test with Prolific test participants | Public URL populated by Prolific |
| Official intake | Recruiting the real study sample | Public URL populated by Prolific |

The icon beside each mode summarizes readiness. Hover over it for the exact blocking conditions. A
green icon means the inputs required by that mode passed their checks.

Start with **Local Preview**. Open the running experiment, exercise the participant flow, and then
select **Stop**. Local Preview does not need a valid Prolific study or API connection, even when
Prolific intake is enabled in the experiment.

Next, select **Test through Prolific** and launch two Prolific test participants if that facility is
available to the workspace. A two-person game cannot test pairing with only one participant. Check
normal completion and at least one non-complete outcome, such as a game that did not start or a
partner disconnect. Select **Stop** when the test is finished. If the workspace does not provide test
participants, use a small pilot with two real submissions instead.

While a run is in progress, **Open** and **Copy URL** always refer to that particular run. Its URL
does not change until the run stops. An experiment cannot be cloned while it is running.

## 7. Open official intake

When the pilot is satisfactory:

1. Confirm that the Prolific study is still linked to the intended Parlando experiment.
2. In Parlando, open **Start experiment** and select **Official intake**.
3. In Prolific, publish or schedule the study.
4. Monitor the first pair from both the Parlando dashboard and Prolific before allowing the full
   sample to proceed.

Start Parlando's official intake before publishing the Prolific study. Otherwise, participants may
reach a Parlando experiment that is not accepting admissions.

Use **Pause intake** in Parlando to stop admitting new participants without marking the experiment
complete. Existing sessions retain their recorded state. Select **Complete** only when collection is
finished and the experiment should no longer reopen for intake.

## 8. Review sessions and submissions

The Parlando session view shows whether a session used Prolific or Direct recruitment. For a
Prolific participant, the compact icon beside the participant name indicates whether Parlando has
checked the current submission status. Select the icon to see the participant, study, submission,
and check details.

Use Parlando to inspect game facts: pairing, participant-specific outcome, waiting duration when it
matters, disconnect or departure cause, and the completion path returned to each participant. Use
Prolific to review submission and payment state. Parlando may read Prolific submission information,
but it does not approve, reject, return, or pay submissions through the API.

## Troubleshooting

### Prolific says the study URL is invalid

Use a public HTTPS deployment. A URL beginning with `http://localhost` is suitable only for Local
Preview and cannot be used as a Prolific external study URL. Keep the three `{{%...%}}` placeholders
literal.

### Parlando cannot retrieve the linked study

Check that the study ID contains only the final ID from the Prolific study-page URL. Confirm that the
API token belongs to a researcher who can access the study's project. Prolific may return `404 Not
Found` when the resource does not exist or the token cannot access it.

### Parlando reports a completion-path conflict

Compare the code and action, not only the path's display name. Recopy the Prolific-generated code and
confirm that its only action matches the table in step 4.

### The timing check fails

The Prolific estimated duration must cover Parlando's maximum partner wait. Prolific's maximum
allowed time must cover Parlando's maximum session lifetime. Increase the Prolific value or reduce
the corresponding Parlando limit, then save the experiment configuration again.

### A participant cannot enter the experiment

Confirm that Parlando intake is currently running, the participant used the linked study, and all
three Prolific identifiers reached the participant URL. For a running study, **Open** and **Copy URL**
in Parlando show the stable URL associated with that run.

For provider-contract details, outcome mappings, signed-launch verification, privacy boundaries,
and restart behavior, see [Prolific integration reference](prolific-integration.md).
