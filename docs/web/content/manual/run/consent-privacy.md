---
title: Prepare consent and privacy settings
---

# Prepare consent and privacy settings

Prepare the participant information before opening research intake. Explain the task, the information participants provide, who receives it, how long it is kept, and who handles questions or withdrawal. Use the [participant information template](../privacy/participant-information-v1.0/) as a starting point and complete it for your actual study.

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
