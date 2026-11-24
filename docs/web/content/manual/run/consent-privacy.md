---
title: Prepare consent and privacy settings
---

# Prepare consent and privacy settings

Prepare the participant information before opening research intake. Explain the task, the information participants provide, who receives it, how long it is kept, and who handles questions or withdrawal. Use the [participant information template](../privacy/participant-information-v1.0/) as a starting point and complete it for your actual study.

## Prepare the consent templates

In **Configuration → Participant access and consent → Consent items**, choose the sections to include using the checkbox list. All 14 informed-consent sections (including separate personal and non-personal information sections) are shown individually with short descriptions, followed by eligibility, research-data consent, communication/privacy rules, and voice/transcription options. Nothing is selected by default. **Select all** checks every section and reveals its text field without overwriting edits. Checking a section reveals its previous template text in an editable text area below it. Replace bracketed placeholders and adapt the wording to your study. Unchecking hides the field and excludes its text from generation, but preserves that writing while the editor remains open.

Choose **Generate consent form** to combine the selected headings and the text you entered, in reading order, into one item titled **Consent Form**. It has one editable identifier, title, statement, and acceptance requirement. With no options checked, the statement is empty. Complete it before saving. Generation keeps existing forms and assigns the new form an unused identifier.

The entire generated form has one required/optional setting; acceptance is required by default. Review the conductor-written text before collecting data. Section fields are temporary drafting inputs; generate the combined form and save a revision to preserve the resulting statement. Participants see the complete plain text before entering the waiting room, and must accept all required items before admission. Save a new configuration revision to persist your forms. Previously saved combined forms are not automatically split.

## Open the study information sites

Creating or cloning an experiment makes four separate information sites available immediately. Open **Privacy → Study documents** and select **Participants**, **Data protection managers**, **Study conductors**, or **Imprint**. Each tab offers **Open page** and **Download HTML**. The **Data documentation** tab retains its generated report and Markdown and JSON downloads. Their paths below the participant study URL are `information/participants/`, `information/data-protection/`, `information/conductors/`, and `information/imprint/`.

These sites are publicly accessible, including while the experiment is inactive, and contain no study records or administrative controls. Configure the imprint under **Configuration → Participant access and consent → Imprint**; saving a revision publishes the edited notice. See [configure the public imprint](configuration/#configure-the-public-imprint). The other three sites have empty HTML bodies and no per-study content editor. Do not use the empty participant site as your approved participant information document; provide a completed document at the information URL configured below.

## Use the approved text in the condition

Open **Configuration** for the inactive experiment and find **Participant access and consent**. Enter the participant information URL and version and provide your approved consent statement. The following excerpt shows the corresponding configuration fields.

```json
{
  "direct": {
    "participant_information_version": "beacon-study-v1",
    "participant_information_url": "https://lab.example.edu/beacon-study",
    "consents": [
      {
        "id": "participate",
        "title": "Consent to participate",
        "body": "I have read the participant information and agree to take part.",
        "required": true
      }
    ]
  }
}
```

Open the participant page during testing and verify the information link, the displayed statement, and what happens when a person declines. A changed consent procedure belongs to a new hosted experiment ID. Keep the information version and the configuration with the study records.

## Verify data handling

The participant's role and recorded events identify a research session. Free text and transcripts may contain identifying information even when export labels are pseudonymous. Review those fields before sharing a corpus. Limit access to the host store and backups, which contain richer operational records than the export.

For participant-facing templates and institutional assessment material, see [Privacy and data protection](../privacy/). See [monitor sessions and export results](data-operations/) for the workflow after collection.
