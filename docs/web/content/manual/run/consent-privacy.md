---
title: Prepare consent and privacy settings
---

# Prepare consent and privacy settings

Prepare the participant information before opening research intake. Explain the task, the information participants provide, who receives it, how long it is kept, and who handles questions or withdrawal. Use the [participant information template](../privacy/participant-information-v1.0/) as a starting point and complete it for your actual study.

## Prepare the consent templates

In **Configuration → Participant access and consent → Consent items**, choose the sections to include using the checkbox list. All 14 informed-consent sections (including separate personal information and pseudonymized research data sections) are shown individually with short descriptions, followed by eligibility, communication/privacy rules, and voice/transcription options. Nothing is selected by default. **Select all** checks every section and reveals its text field without overwriting edits. Checking a section reveals its previous template text in an editable text area below it. Replace all `{{…}}` placeholders and adapt the wording to your study. Unchecking hides the field and excludes its text from generation, but preserves that writing while the editor remains open.

Choose **Generate consent form** to combine the selected headings and the text you entered, in reading order, into one item titled **Consent Form**. It has one editable identifier, title, statement, and acceptance requirement. With no options checked, the statement is empty. Complete it before saving. Generation keeps existing forms and assigns the new form an unused identifier.

The entire generated form has one required/optional setting; acceptance is required by default. Review the conductor-written text before collecting data. Section fields are temporary drafting inputs; generate the combined form and save a revision to preserve the resulting statement. Participants see the complete plain text before entering the waiting room, and must accept all required items before admission. Save a new configuration revision to persist your forms. Previously saved combined forms are not automatically split.

The default templates describe a two-week period after participation during which research records remain pseudonymized under a random study-specific ID and can be located for participant requests. They assign subsequent anonymization to the research team, including removal of recruitment links and review of identifying content. Once anonymization is complete, the team cannot identify a participant's contributions from their identity. Separately retained payment, consent, and other personal records require their own documented retention and rights procedures.

This wording is a study policy, not an automatic Parlando retention feature. Parlando does not schedule two-week anonymization, review transcripts for identifying information, or remove copies in exports and backups automatically. Before using the policy, arrange the operational process, resolve pending participant requests, and complete the storage, provider, retention, and contact placeholders. Save validation rejects unresolved `{{…}}` placeholders.

## Open the study information sites

Creating or cloning an experiment makes four separate information sites available immediately. Open **Privacy → Study documents** and select **Participants**, **Data protection managers**, **Study conductors**, or **Imprint**. Each tab offers **Open page** and **Download Markdown**. The **Data documentation** tab retains its generated report and Markdown and JSON downloads. Their paths below the participant study URL are `information/participants/`, `information/data-protection/`, `information/conductors/`, and `information/imprint/`.

These sites are publicly accessible, including while the experiment is inactive, and contain no study records or administrative controls. Configure the imprint under **Configuration → Participant access and consent → Imprint**; saving a revision publishes the edited notice. See [configure the public imprint](configuration/#configure-the-public-imprint). The Participants, Study conductors, and Data protection managers tabs use the configured consent forms and preview them, including unsaved edits. Saving a configuration revision publishes their titles and statements on all three audience information pages and in their Markdown downloads. When no statement is configured, the page displays “Please configure the informed consent form.” Changes to the configured consent form apply to all three audiences.

The Study conductors page omits **Subject of this study**, **Possible benefit of the study**, **Declaration of consent**, and **Communication and privacy rules**. The participant page retains these sections. Sections are recognized by their generated heading at the start of the statement or after a blank line; keep those headings intact when editing the combined statement for audience filtering to apply. Custom headings remain visible. Template changes apply when generating a new form; regenerate and save to update existing consent copy.

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

The Data protection managers page includes **Data processing** (purposes and legal basis), **Personal information collected**, **Pseudonymized research data collected**, **Data controller and contact person**, **Data storage**, and **Disclosure and publication**. It links to the full participant informed consent form. Its public page and Markdown download use the same selection and an absolute link to the participant page. Keep generated headings intact so the sections can be recognized; separately configured forms with one of these section titles are also included. Unclassified text is omitted from this view.

## Use the study conductor TODOs

The study conductor preview, public page, and Markdown download include **TODO for study conductors** when a consent statement is configured. Tasks are selected from the original form’s recognized section headings, including participant sections omitted from the conductor summary. Each group appears once. Access/correction/deletion, withdrawal, and retention sections bring in request handling and the template’s two-week anonymization responsibilities, including removal of recruitment links, review of identifying content, and management of exports and backups. Other sections add their relevant collection, access, provider, publication, consent, and contact duties.

The preview follows generated-form edits; public pages and downloads reflect the saved revision. These are instructions, not automatic actions or stored completion records. Markdown exports use unchecked task items for use in your own workflow. Review custom commitments and changed deadlines manually: the mapping recognizes template headings, not arbitrary promises in edited prose. Keep headings intact for the relevant tasks to appear.

The research-data and storage templates include `{{RESEARCH_DATA_STORAGE_LOCATION_AND_COUNTRY}}`, visible in the data protection document when either section is included. Complete the hosting-log policy in Personal information collected from the actual hosting configuration: Parlando does not write participant IP addresses to its research database or corpus exports, but external infrastructure may keep separate logs. The subject template discloses possible human and AI-chatbot interaction, with a conductor TODO to explain the actual partners before participation. Research-data and publication templates describe further scientific use and publication after verified anonymization; complete the associated recipients, publication route, and license details.
