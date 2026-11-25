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

## Configure the public imprint

Under **Participant access and consent → Imprint**, select the country where the
study operator is established. The selector provides templates for Germany, the
United States, France, Austria, Spain, Ireland, Italy, and the Netherlands, plus a
general EU starting point. Each generated imprint names the selected template and
lists its legal references with official source URLs, qualified by applicability
to the operator. Standalone HTTPS source URLs are clickable in the preview, public
page, and Markdown download. The templates omit German and Austrian media-law references. The US
template explains the absence of a general federal imprint requirement. The EU
option does not replace national requirements. Existing saved text is preserved;
to use an updated template, select another country and then the intended country
again, which replaces the current text.

The operating institution is pre-filled from **Shared settings → Institution**.
Enter the responsible contact person's name, public postal address, email, and
telephone. These fields populate the template; they do not identify the contact
person as the institution's legal representative. Complete the remaining
placeholders and remove inapplicable lines. The templates assume a public
annotation study without selling or editorial publishing and require review for
the operator's legal form and applicable national requirements.

Contact changes update linked values within the imprint while preserving custom
wording around them. Selecting a different country replaces the text with that
country’s template, using the current shared contact fields. Existing unlinked
free-text notices are preserved.

**Privacy → Imprint** displays the current imprint immediately, including unsaved
edits. A draft label distinguishes unsaved text from the saved public notice.

**Save new revision** publishes the exact imprint text at the study's
`information/imprint/` URL, including while intake is closed. **Privacy → Imprint**
provides the public link and a Markdown download of the same notice. Changes to shared
institution settings do not rewrite an already saved notice. Keep a separate
privacy notice describing the study's processing of personal data.

### Edit or add imprint template files

Each country’s complete imprint text is a Markdown file in `rust-server/templates/imprint/`. The server discovers `.md` files recursively at startup and uses them for the country selector and generation. Restart after editing templates; no rebuild is needed for template-only changes.

Files support YAML front matter with `id` (a unique two-letter uppercase country code, or `EU`), `title` (the selector label), and `order` (lower values first). Without metadata, a file such as `BE.md` uses `BE` as its ID and derives a label from a leading Markdown heading or the filename. Invalid metadata or duplicate IDs/titles stop startup. Keep template files for countries referenced by saved configurations: country validation uses the loaded catalogue.

The Markdown body contains the complete editable notice, including selected-template wording, legal references and source links. Contact-field substitution uses `{{field|fallback text}}` for `operator`, `address`, `responsible_person`, `email`, and `phone`. For example:

```markdown
---
id: DE
title: Germany
order: 10
---

Operator: {{operator|[full legal name]}}
Address: {{address|[full postal address]}}
```

The fallback remains visible when the contact field is empty. Other placeholders remain editable text. Existing generated notices, manual edits, previews and publication behavior are unchanged.

For deployed or relocated binaries, copy the folder and set `PARLANDO_IMPRINT_TEMPLATE_DIR` before startup. By default, Parlando uses `templates/imprint/` beneath the Rust crate source directory used at build time. The supplied container images include the files under `/usr/local/share/parlando/imprint/` and configure this variable.

## Open and close intake

| Intake state | Use it for |
| --- | --- |
| Inactive | Keep the condition closed to new participants. |
| Testing | Run a pilot. Testing sessions appear in monitoring and are excluded from research corpus exports. |
| Active | Admit research participants under the selected condition. |

Open **Start experiment** and start **Local preview** for the pilot. Play both seats in separate browser profiles and check the resulting sessions in the dashboard. Use **Official intake** for data collection and distribute the URL from the running experiment. Record the game package revision, experiment ID, and recruitment link together.

Use **Pause intake** when recruitment ends. Keep the host running while participants finish. On restart, reopen intake explicitly. See [monitor sessions and export results](data-operations/) before archiving a study.

## Configure the privacy notice

Under **Participant access and consent → Privacy notice**, select **EU/EEA**,
**California**, **UK**, **Switzerland**, **Canada**, or **Brazil**. These are
jurisdiction-specific starting points with official source links. Coverage depends
on the operator and processing, and several laws can apply at once. In particular,
the Canadian template requires review of provincial and public-sector rules.

Generate and edit the consent forms, and complete the imprint first. The notice
copies recognized privacy-related consent sections and the public imprint text
and contact fields. It does not infer a legal controller from an operator name or
a legal basis from participation consent. Complete the additional disclosures
shown beneath the selector, including any relevant custom consent sections that
were not recognized. Unfilled `{{placeholders}}` remain visible and trigger a
review message; saving a draft does not certify its completeness.

The generated notice is read-only. Edit its source forms or supplemental fields
to change it. Matching fields share their values across all three document editors. Consent-form edits, generation/removal, imprint edits and jurisdiction
changes immediately update the preview. Switching jurisdictions preserves the
supplemental fields. Checkbox section text becomes a source after **Generate
consent form**. The preview uses the current runtime templates when configuration
opens; the public page continues to show the saved revision until you save again.

**Save new revision** publishes the snapshot at `information/privacy/`.
**Privacy notice → Privacy notice** provides the preview, public link and Markdown
download. An unconfigured public page asks the conductor to configure the notice.
The participant footer links **Informed consent**, **Privacy notice**, and
**Imprint** throughout participation. Rebuild and upload game client bundles to
adopt the updated SDK footer.

### Edit privacy notice template files

Each Markdown file in `rust-server/templates/privacy-notice/` supplies one selector
option using the same `id`, `title`, `description`, and `order` front matter as the
consent catalogue. The server reads the directory once at startup. Restart after
changing files, and save a new experiment revision to publish regenerated copy.
`PARLANDO_PRIVACY_NOTICE_TEMPLATE_DIR` overrides the directory; the default is
`templates/privacy-notice/` under the Rust crate source directory used at build
time. Supplied container images include the catalogue.

`{{source.imprint}}` and `{{source.consent}}` insert the synchronized public source
blocks. Other placeholders use lowercase letters, digits and underscores, starting
with a letter, for example `{{local_disclosure}}`; each creates an editable public
text field. Inserted values are literal and are not interpreted as further template
instructions. Keep template identifiers stable while saved studies reference them.

### Enter repeated information once

Consent sections, imprint identity controls and privacy disclosures share answers
for matching facts. Enter a value beside a consent section, in the imprint, or in
the privacy notice fields (including **Shared study information**). Corresponding
fields and linked draft text update immediately. The operator, legal controller,
public study contact and rights-request contact remain distinct roles.

Repeated placeholders, such as research-data storage location or transcription
provider, use one answer even across several consent sections. Generate the consent
form as usual. The saved revision retains both the rendered public text and its
links, so later edits continue to update the forms after reopening configuration.
Editing only a linked value inside generated text updates the shared answer;
editing surrounding prose preserves the remaining links. Replacing a passage that
contains several fields makes that passage local text rather than guessing which
facts changed. Clearing a shared answer restores unresolved placeholders wherever
that answer is required. Optional supplemental collection/storage details may be
left empty when the source sections are complete.

Previously written free-text documents are preserved rather than parsed for
meaning. Regenerate those documents from the current templates to establish links.
Conflicting saved field values are flagged; editing the relevant shared field
explicitly chooses the value to use throughout the draft. **Save new revision**
publishes the rendered snapshots; editing shared fields does not alter an already
published revision or previously recorded consent evidence.

Custom consent placeholders also create shared fields. Keys are normalized to
lowercase with punctuation converted to underscores. Use the same placeholder
name wherever the same fact is needed, and distinct names for different facts.
For example, `{{research_data_storage_location_and_country}}` shares the built-in
storage location. `{{optional_detail|}}` supplies an empty default for an optional
fact. Inserted answer text is literal, not recursively interpreted as a template.
