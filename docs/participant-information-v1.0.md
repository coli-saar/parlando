<!--
Maintainer instructions (not part of the participant-facing page):

1. Replace every {{PLACEHOLDER}} before publication.
2. Remove statements for features that are not enabled.
3. Insert the Markdown/JSON privacy-status facts from the running installation.
4. If consent is not the legal basis, replace the legal-basis and withdrawal text.
5. Give the deployed page its own stable version and URL. Any material text change
   requires a new local version even when this Parlando template remains v1.0.
6. Do not publish this file with unresolved placeholders.
-->

# Participant Information and Privacy Notice

**Study:** {{STUDY_TITLE}}<br>
**Responsible institution:** {{INSTITUTION_NAME}}<br>
**Local document version:** {{LOCAL_INFORMATION_VERSION}}<br>
**Based on Parlando participant-information template:** 1.0, 14 August 2026

Please read this page before deciding whether to participate. You may save or print a copy.

## What is this study about?

In this study, you will interact with another participant or a software-controlled player in a fictional game. We study communication, coordination and decision-making within the game. We do not ask you to discuss your real life, health, political views or other sensitive personal matters.

The research team is {{RESEARCH_UNIT}} at {{INSTITUTION_NAME}}. The study is led by {{PRINCIPAL_INVESTIGATOR}}.

## What will I do?

You will be assigned a role and complete a fictional task with another role. Depending on the study condition, you may communicate through game actions, typed chat or live voice. The session is expected to last approximately {{SESSION_DURATION}}.

The other role may be controlled by another participant or by a trusted software agent operated as part of the experiment. The software agent receives only the game view and communications assigned to its role.

## Is participation voluntary?

Yes. Participation is voluntary. You may decline or stop at any time without giving a reason and without disadvantage. {{PAYMENT_AND_EARLY_STOPPING_RULE}}

You must be at least 18 years old to participate in this study.

## What data does Parlando process?

Parlando automatically assigns you a randomly generated, human-readable participant identifier for this study. You are not asked to enter a name. If you participate in another Parlando study, you receive a different identifier there. If a recruitment platform is used, the recruitment identifier is kept separately from normal research and corpus exports.

The running study is configured as follows:

- Complete game states stored: **{{STORE_FULL_GAME_STATE_YES_NO}}**
- Typed participant messages stored: **{{STORE_TYPED_MESSAGES_YES_NO}}**
- Final voice transcripts stored: **{{STORE_FINAL_TRANSCRIPTS_YES_NO}}**
- Minimized voice diagnostics stored: **{{STORE_VOICE_DIAGNOSTICS_YES_NO}}**
- Live voice enabled: **{{VOICE_ENABLED_YES_NO}}**
- Hosted transcription enabled: **{{TRANSCRIPTION_ENABLED_YES_NO}}**

Depending on those settings, the research data may include:

- study-specific identifiers, role and session status;
- game actions, events, timing and results;
- game state required to reconstruct the fictional task;
- messages you type in the study;
- final machine-generated transcripts of speech; and
- limited technical error codes and measurements needed to diagnose voice problems.

Parlando does not store raw microphone audio. It does not store microphone device identifiers or labels. Infrastructure operated by {{INSTITUTION_NAME}} may process IP addresses and ordinary access logs for {{INFRASTRUCTURE_LOG_RETENTION_AND_PURPOSE}}.

## Who receives my communications?

The other role receives the game events and communications intended for that role. If live voice is enabled, the other participant can hear your voice. Please do not record or redistribute another participant's communications. The research team cannot completely prevent a participant from making an independent recording or screenshot.

If hosted transcription is enabled, live microphone audio is sent to {{SPEECHMATICS_ENTITY_AND_SERVICE}} in {{SPEECHMATICS_PROCESSING_REGION}} for real-time transcription. Parlando receives the resulting text and timing information. Parlando does not provide its raw-audio stream to any other standard service. Further information about provider retention and transfers: {{SPEECHMATICS_RETENTION_AND_TRANSFER_INFORMATION}}.

You may hear a synthetic software-agent voice. The text-to-speech service receives only text written by the software agent. It does not receive your microphone audio, messages, transcripts, identifier or game state.

Within {{INSTITUTION_NAME}}, access is restricted to {{AUTHORISED_RESEARCH_AND_ADMIN_ROLES}}. Parlando is self-hosted by or for {{INSTITUTION_NAME}}; the Parlando developers and Saarland University do not receive data merely because the software is used.

## How will the research data be used?

The data will be used for:

- analysis of this study;
- verification and reproducibility of the results;
- subsequent scientific research on interaction, dialogue and game behaviour; and
- preparation and publication of anonymized research corpora.

Internal and controlled research data are pseudonymized and remain personal data while the participant identifier for this study can still be linked to your recruitment record. They are not published with recruitment identifiers or the internal identifiers used by the live system. The randomly generated participant and dialogue identifiers remain unchanged in repeated exports of this study so that its research datasets can be compared and extended. They are not reused to identify you in another study.

The corpus export removes internal system identifiers and recruitment information and converts absolute timestamps to relative timing. It retains the randomly generated participant and dialogue labels so that repeated exports of this study remain consistent. Before public release, the research team also removes the link to recruitment records and reviews dialogue content for names, contact details, real institutions, locations and other identifying information. A corpus is released publicly only when the research team concludes that participants are no longer identifiable by means reasonably likely to be used. Otherwise the data remain under controlled access.

Once data have been genuinely anonymized, the research team can no longer determine which participant label or contribution was yours. Random labels may remain in the corpus, but the link between you and a label no longer exists. Your individual contribution can then no longer be located or withdrawn on the basis of your identity.

## What should I avoid sharing?

Please discuss only the fictional task. Do not disclose addresses, contact details, passwords, health information, political or religious beliefs, sexual orientation, criminal allegations or other sensitive real-world information.

If you accidentally disclose identifying or sensitive information, contact the research team as soon as possible so that it can be reviewed and, where possible, removed before anonymization or publication.

## How long will the data be kept?

The participant-to-recruitment link, pseudonymized research data and consent record are kept according to the following local schedule:

{{RETENTION_SCHEDULE}}

Deletion is initiated manually by the authorised research or administrative team. It is not performed automatically by Parlando.

## Legal basis and withdrawal

{{LEGAL_BASIS_TEXT}}

If processing is based on your consent, you may withdraw that consent at any time with effect for the future by contacting {{WITHDRAWAL_CONTACT_AND_PROCEDURE}}. Processing carried out before withdrawal remains lawful. Withdrawal is possible while the research team can still link the data to your study-specific participant identifier. It is no longer possible after the relevant data have been genuinely anonymized.

## Risks

This is a low-risk fictional game study. The main foreseeable privacy risks are accidentally sharing real-world information, being recognized by a voice partner, an inaccurate transcript, or unauthorized access following a security incident. We reduce these risks through study-specific identifiers, encrypted communication, restricted access, no raw-audio storage, data-minimized exports and review before corpus publication.

The study does not use your data to make legal, educational, employment or similarly significant decisions about you.

## Your data-protection rights

Depending on the legal basis and applicable research law, you may have rights of access, correction, deletion, restriction, objection and data portability, as well as the right to withdraw consent. To exercise a right, contact {{DATA_REQUEST_CONTACT}} and provide {{RESEARCH_CODE_PROCEDURE}}. You may also contact the institution's Data Protection Officer or lodge a complaint with a competent supervisory authority.

## Contacts

**Study and data requests**<br>
{{RESEARCH_CONTACT_NAME}}<br>
{{RESEARCH_CONTACT_UNIT_AND_ADDRESS}}<br>
{{RESEARCH_CONTACT_EMAIL_AND_PHONE}}

**Controller**<br>
{{INSTITUTION_CONTROLLER_DETAILS}}

**Data Protection Officer**<br>
{{INSTITUTION_DPO_DETAILS}}

**Competent supervisory authority**<br>
{{SUPERVISORY_AUTHORITY_DETAILS}}
