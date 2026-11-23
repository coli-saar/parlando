//! Interprets scoped configuration facts for the report builder. A single configuration
//! produces individual setting descriptions. Across configurations, provider identities are
//! deduplicated separately for transcription and synthesis because those services receive
//! different participant data.

use std::collections::HashSet;

use super::{facts::PrivacyConfigFacts, PrivacyConfigurationStatus, PrivacyServiceStatus};

/// Describes services that receive data across the selected configurations.
pub(super) fn external_services(facts: &[PrivacyConfigFacts]) -> Vec<PrivacyServiceStatus> {
    let mut external_services = Vec::new();
    let mut transcription_providers = HashSet::new();
    let mut synthesis_providers = HashSet::new();
    for fact in facts {
        if fact.transcription_enabled
            && transcription_providers.insert(fact.transcription_provider.as_str())
        {
            external_services.push(PrivacyServiceStatus {
                service: fact.transcription_provider.clone(),
                enabled: true,
                purpose: "Convert participant speech to text during the live session."
                    .to_string(),
                data_sent: "Live microphone audio. The provider returns transcript text and timing information to Parlando.".to_string(),
            });
        }
        if fact.tts_enabled && synthesis_providers.insert(fact.tts_provider.as_str()) {
            external_services.push(PrivacyServiceStatus {
                service: fact.tts_provider.clone(),
                enabled: true,
                purpose: "Convert a software agent's message to speech for participants."
                    .to_string(),
                data_sent: "The software agent's message text. The request does not contain participant audio, participant identifiers, or game state as separate fields. Because agent-generated text can repeat information from the conversation, the message itself must still be treated as session content.".to_string(),
            });
        }
    }
    external_services
}

/// Describes individual settings for one configuration, or summarizes the covered set.
pub(super) fn configuration(facts: &[PrivacyConfigFacts]) -> Vec<PrivacyConfigurationStatus> {
    let total = facts.len();
    let selected = (total == 1).then(|| &facts[0]);
    if let Some(fact) = selected {
        let participant_information = if fact.participant_information_url.is_empty()
            && fact.participant_information_version.is_empty()
        {
            PrivacyConfigurationStatus {
                setting: "Participant information".to_string(),
                status: "Not configured".to_string(),
                detail: "This experiment has no participant-information document URL or version in its Parlando configuration."
                    .to_string(),
            }
        } else {
            PrivacyConfigurationStatus {
                setting: "Participant information".to_string(),
                status: "Configured".to_string(),
                detail: format!(
                    "Version: {}; document: {}.",
                    if fact.participant_information_version.is_empty() {
                        "not specified"
                    } else {
                        &fact.participant_information_version
                    },
                    if fact.participant_information_url.is_empty() {
                        "URL not specified"
                    } else {
                        &fact.participant_information_url
                    }
                ),
            }
        };
        vec![
            PrivacyConfigurationStatus {
                setting: "Participant arrangement".to_string(),
                status: if fact.human_vs_agent {
                    "One human and one software agent"
                } else {
                    "Two human participants"
                }
                .to_string(),
                detail: if fact.human_vs_agent {
                    "Each session assigns one human participant and one configured software agent."
                } else {
                    "Each session assigns two human participants; this experiment does not create software-agent participant records."
                }
                .to_string(),
            },
            PrivacyConfigurationStatus {
                setting: "Voice communication".to_string(),
                status: if fact.voice_enabled { "Enabled" } else { "Disabled" }.to_string(),
                detail: if fact.voice_enabled {
                    "The browser sends microphone audio to Parlando for live voice communication."
                } else {
                    "The browser does not start Parlando voice capture; Parlando therefore does not receive, relay, or store participant microphone audio for this experiment."
                }
                .to_string(),
            },
            PrivacyConfigurationStatus {
                setting: "Speech transcription".to_string(),
                status: if fact.transcription_enabled { "Enabled" } else { "Disabled" }.to_string(),
                detail: if fact.transcription_enabled {
                    format!("Participant microphone audio is sent to {} at {} for live speech-to-text processing.", fact.transcription_provider, fact.transcription_endpoint)
                } else {
                    "No participant audio is sent to a transcription provider and no speech transcripts are produced."
                        .to_string()
                },
            },
            PrivacyConfigurationStatus {
                setting: "Agent speech synthesis".to_string(),
                status: if fact.tts_enabled { "Enabled" } else { "Disabled" }.to_string(),
                detail: if fact.tts_enabled {
                    format!("Software-agent message text is sent to {} at {} to generate speech.", fact.tts_provider, fact.tts_endpoint)
                } else {
                    "No text is sent to a speech-synthesis provider."
                        .to_string()
                },
            },
            PrivacyConfigurationStatus {
                setting: "Consent items".to_string(),
                status: if fact.consent_items == 0 {
                    "None configured".to_string()
                } else {
                    format!("{} configured", fact.consent_items)
                },
                detail: if fact.consent_items == 0 {
                    "Parlando does not collect consent declarations for this experiment."
                        .to_string()
                } else {
                    "The configured declarations are presented and recorded as described below."
                        .to_string()
                },
            },
            participant_information,
        ]
    } else {
        vec![PrivacyConfigurationStatus {
            setting: "Experiments covered".to_string(),
            status: total.to_string(),
            detail: "This installation-wide endpoint summarizes multiple experiment configurations; use an experiment Privacy tab for an experiment-specific record."
                .to_string(),
        }]
    }
}
