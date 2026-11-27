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

## Create shared document templates

After signing in, open **Templates** next to **Games** and **Users**. Give the
shared template a name and configure its informed consent, imprint, and privacy
notice. Choose **Save template** to store the reusable set. Signed-in lab members
share this library; edits here do not change published experiment pages.

In an experiment, open **Configuration → Participant access and consent → Templates**,
select a saved template, and choose **Apply template**. This copies only its three
documents into the experiment draft. Review them in the existing **Privacy notice**
tab and choose **Save new revision** to publish the copy. Applying a different
template replaces the draft documents. Subsequent changes to the library do not
alter previously applied copies or other experiment settings.

Existing experiment documents are retained. Use **Copy current documents to
Templates** to give them a reusable name and edit the shared copy in **Templates**.

## Generate informed consent

A new shared template already contains the complete informed-consent form. There
is no section selector or generation step. Enter name, institute, email, end date,
license, data platform, payment, and server once under **Templates → Informed consent**.
Matching placeholders in the consent, imprint, and privacy notice update together,
including aliases such as `{{institution}}` and `{{institute}}` or `{{email}}` and
`{{public contact email}}`. Empty fields leave the corresponding placeholders visible.

Only **Personal information collected** and **Other research data** have consent
text editors. The rest of the form is a read-only generated preview based on the
Markdown catalogue. Remaining TODOs or placeholders outside the eight shared fields
must be completed in the source templates before creating a new draft. Editing a
filled occurrence directly makes that occurrence independent of its shared field.
Country-specific notice text remains editable for details outside those fields.

Before entering the waiting room, participants see the statement rendered as
Markdown. Paragraphs beginning “I have read and adhere”, “I confirm that I am”,
and “I agree to participate” each receive an acknowledgement checkbox. All present
declarations must be checked to accept that form. If none of these declarations
is present, one acceptance checkbox appears below the statement. Acceptance is
recorded for the complete form, rather than separately for each paragraph.

Choose **Save template** to retain both the completed statement and its editable
field values. Apply the template to an experiment and save a new revision to publish it. Existing statements without
field metadata can fill remaining placeholders, but values already written into
ordinary prose are not inferred.

## Configure the public imprint

Under **Templates → Imprint**, select the operator's country.
The editor has one country selector and one text area. Replace the `{{placeholders}}`
directly in the notice and remove inapplicable lines. The templates include the
selected country, applicable legal references and official source links.

Selecting another country replaces the draft with that template. Typing, opening
the library again, or editing unrelated document text does not overwrite the notice.
Shared study fields update their linked occurrences in the notice.
After applying the template to an experiment, **Save new revision** publishes its
exact text at `information/imprint/`.
**Privacy notice → Imprint** provides the preview, public link and Markdown download.

### Edit or add imprint template files

Templates live in `rust-server/templates/imprint/`. Restart the server after editing
them. YAML front matter accepts `id` (a unique two-letter uppercase code, or `EU`),
`title`, `description`, and `order`. Keep identifiers used by saved studies.

The body is the complete editable notice. Placeholders matching the eight shared
study fields are filled automatically; other placeholders require manual completion:

```markdown
---
id: DE
title: Germany
order: 10
---

Operator: {{Full legal name of the operator}}
Address: {{Full postal address, including country}}
```

Set `PARLANDO_IMPRINT_TEMPLATE_DIR` to use another directory. The default is
`templates/imprint/` beneath the Rust crate source directory used at build time;
supplied container images include the catalogue and configure its location.

## Open and close intake

| Intake state | Use it for |
| --- | --- |
| Inactive | Keep the condition closed to new participants. |
| Testing | Run a pilot. Testing sessions appear in monitoring and are excluded from research corpus exports. |
| Active | Admit research participants under the selected condition. |

Open **Start experiment** and start **Local preview** for the pilot. Play both seats in separate browser profiles and check the resulting sessions in the dashboard. Use **Official intake** for data collection and distribute the URL from the running experiment. Record the game package revision, experiment ID, and recruitment link together.

Use **Pause intake** when recruitment ends. Keep the host running while participants finish. On restart, reopen intake explicitly. See [monitor sessions and export results](data-operations/) before archiving a study.

## Configure the privacy notice

Under **Templates → Privacy notice**, select **EU/EEA**,
**California**, **UK**, **Switzerland**, **Canada**, or **Brazil**. Each option loads
a complete template into a single editable text area. Replace the descriptive
`{{placeholders}}` with the study's information, including contacts, data collected,
purposes, storage, retention, recipients, cookies and jurisdiction-specific details.
The eight shared study fields also fill matching placeholders here. Other disclosure
details remain directly editable in the notice.

Templates are starting points with official source links. Applicable requirements
depend on the operator and processing; several laws may apply. Review the completed
notice before publishing. Changing the jurisdiction replaces the draft; reopening
configuration and editing other documents preserve the text.

After applying the template to an experiment, **Save new revision** publishes the
notice at `information/privacy/`.
**Privacy notice → Privacy notice** provides its preview, public link and Markdown
download. An unconfigured public page asks the conductor to configure the notice.
The participant footer links to informed consent, privacy notice and imprint.

### Edit privacy notice template files

Each Markdown file in `rust-server/templates/privacy-notice/` supplies one selector
option, using `id`, `title`, `description`, and `order` front matter. Its body is
loaded verbatim; `{{placeholders}}` are instructions to replace directly in the
editor. Matching placeholders use the existing shared fields; no additional fields
are created.

The server reads the directory at startup. Restart after changing files, then
select the template and save a new revision to publish updated copy. Existing
saved text is not regenerated automatically. `PARLANDO_PRIVACY_NOTICE_TEMPLATE_DIR`
overrides the directory; the default is `templates/privacy-notice/` beneath the
Rust crate source directory used at build time. Supplied container images include
the catalogue. Keep identifiers stable while saved studies reference them.

### Format study documents with Markdown

Consent, imprint and privacy-notice previews and public HTML pages render headings
(`#` through `######`), bold/italic text, links, flat bullet/numbered/task lists,
quotes, horizontal rules and fenced code blocks. Existing plain-text line breaks
remain visible. Raw HTML is displayed as text; links support HTTP(S), email and
relative destinations. `{{placeholders}}` remain literal until you replace them.
Tables, nested lists and embedded images are not supported by this lightweight renderer.

Public pages use a locally served script for formatting. With JavaScript disabled,
the readable source text remains visible. Saving and Markdown downloads retain the
original text; displaying a document does not modify a revision or consent record.
