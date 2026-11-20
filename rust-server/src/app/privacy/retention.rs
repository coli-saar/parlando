//! Builds the stored-data and transient-processing sections of the privacy report. It uses
//! voice, transcription, consent and agent configuration to distinguish which data can be
//! produced in a single experiment and to summarize mixed installations.

use super::{facts::PrivacyConfigFacts, PrivacyNonRetentionStatus, PrivacyStorageStatus};

/// Lists the data written by the configured features.
pub(super) fn stored_data(
    facts: &[PrivacyConfigFacts],
    transcription: usize,
    consent_items: usize,
    provider_description: &str,
    has_external_services: bool,
) -> Vec<PrivacyStorageStatus> {
    let total = facts.len();
    let selected = (total == 1).then(|| &facts[0]);
    let consent_storage_detail = if total == 1 && consent_items == 0 {
        "No consent items are configured for this experiment, so Parlando does not create consent declaration records."
            .to_string()
    } else {
        "For each declaration, Parlando stores the consent item, whether it was accepted, declaration time, research or testing purpose, a cryptographic fingerprint of the complete text shown to the participant, and consent metadata. A fingerprint can verify unchanged text but does not contain the text itself; the configured consent text is retained in the versioned experiment configuration described above."
            .to_string()
    };
    let transcript_storage_detail = if total == 1 && transcription == 0 {
        "Speech transcription is disabled. Parlando does not generate or store voice-transcript events for this experiment."
            .to_string()
    } else if total == 1 {
        format!(
            "Speech transcription through {provider_description} is enabled. Parlando stores event order, milliseconds from game start, speaking participant and role, final transcript text, and speech start and end positions on the same game clock, and independently timed spoken tokens with optional recognition confidence. It does not store interim recognition hypotheses."
        )
    } else {
        format!(
            "Speech transcription is enabled in {transcription} of {total} covered experiments. Those experiments retain event order, milliseconds from game start, speaking participant and role, final transcript text, and speech start and end positions on the same game clock, and independently timed spoken tokens with optional recognition confidence; interim recognition hypotheses are not stored."
        )
    };
    let mut operational_sources = vec!["session lifecycle", "connection changes"];
    if facts.iter().any(|fact| fact.human_vs_agent) {
        operational_sources.push("software-agent status");
    }
    if has_external_services {
        operational_sources.push("enabled external speech-service status");
    }
    let operational_event_detail = format!(
        "Events for {} are retained with event order and game_time_ms, a signed millisecond coordinate whose zero is game start. For rejected action input, Parlando stores a reason, byte count, and cryptographic fingerprint rather than the rejected content. Operational events have no output field in the corpus unless the event is an accepted action or participant message.",
        operational_sources.join(", ")
    );
    vec![
            PrivacyStorageStatus {
                category: "Participant records".to_string(),
                persisted_when_produced: true,
                purpose: if selected.is_some_and(|fact| fact.human_vs_agent) {
                    "Distinguish the human participant and software agent and link both to their session."
                } else {
                    "Distinguish the two human participants and link them to their session."
                }
                .to_string(),
                detail: if selected.is_some_and(|fact| fact.human_vs_agent) {
                    "When the human participant record is created, Parlando randomly generates a three-word pseudonym in the form adverb-adjective-animal, retries if it collides with an existing pseudonym, and stores it with this experiment. The pseudonym is not derived from a name, contact detail, participant IP address, or other external identifier, and Parlando does not use it to match the person across experiments. Parlando also stores that this is a human record and its creation time; the session links it to the software-agent participant."
                } else {
                    "When each participant record is created, Parlando randomly generates a three-word pseudonym in the form adverb-adjective-animal, retries if it collides with an existing pseudonym, and stores it with this experiment. The pseudonym is not derived from a name, contact detail, participant IP address, or other external identifier, and Parlando does not use it to match the person across experiments. Parlando also stores that this is a human record and its creation time."
                }
                .to_string(),
            },
            PrivacyStorageStatus {
                category: "Sessions and assignments".to_string(),
                persisted_when_produced: true,
                purpose: "Operate a two-participant session, support reconnection, and preserve its experimental context and outcome."
                    .to_string(),
                detail: "For each session, Parlando randomly generates a three-word pseudonym in the form adverb-adjective-place-or-object. It also stores internal numeric and public session identifiers; temporary participant-session identifiers used for access and reconnection; research or testing purpose; participant roles; connection status; creation, start, completion, join, and leave times; final status; and the game-supplied completion record."
                    .to_string(),
            },
            PrivacyStorageStatus {
                category: "Consent evidence".to_string(),
                persisted_when_produced: consent_items > 0,
                purpose: "Show which version of the participant information and consent text was presented and accepted."
                    .to_string(),
                detail: consent_storage_detail,
            },
            PrivacyStorageStatus {
                category: "Accepted actions and game state".to_string(),
                persisted_when_produced: true,
                purpose: "Reconstruct the experimental interaction and analyze game behavior and outcomes."
                    .to_string(),
                detail: "Event order and game_time_ms, measured in milliseconds from game start; acting participant and role; accepted action; game-supplied information about how the action changed the game; and the complete game state after the action."
                    .to_string(),
            },
            PrivacyStorageStatus {
                category: "Game and agent logs".to_string(),
                persisted_when_produced: true,
                purpose: "Preserve implementation-defined diagnostics attached to the session."
                    .to_string(),
                detail: "Games and software agents may store arbitrary text with game or runtime-assigned agent attribution. Parlando bounds log size but does not inspect, redact, or otherwise constrain the text's meaning; extension authors must avoid logging credentials or unnecessary personal data."
                    .to_string(),
            },
            PrivacyStorageStatus {
                category: "Typed participant messages".to_string(),
                persisted_when_produced: true,
                purpose: "Preserve the dialogue as experimental data."
                    .to_string(),
                detail: "Event order and game_time_ms, measured in milliseconds from game start; sending participant and role; message text; an indication that it was typed; and message metadata."
                    .to_string(),
            },
            PrivacyStorageStatus {
                category: "Final voice transcripts".to_string(),
                persisted_when_produced: transcription > 0,
                purpose: if transcription == 0 {
                    "Not applicable because speech transcription is disabled.".to_string()
                } else {
                    "Preserve spoken dialogue as text for experiments with speech transcription enabled."
                        .to_string()
                },
                detail: transcript_storage_detail,
            },
            PrivacyStorageStatus {
                category: "Agent speech timings".to_string(),
                persisted_when_produced: facts.iter().any(|fact| fact.tts_enabled && fact.human_vs_agent),
                purpose: "Analyze generated agent speech alongside participant speech.".to_string(),
                detail: "Agent message text, participant and role, estimated speech start and end on the game clock, and spoken words and punctuation from synthesis alignment. Timings are anchored to server audio publication; network and browser buffering can shift playback. Generated audio is not stored, and agent tokens have no recognition confidence.".to_string(),
            },
            PrivacyStorageStatus {
                category: "Operational session events".to_string(),
                persisted_when_produced: true,
                purpose: "Diagnose session failures and establish what happened during collection."
                    .to_string(),
                detail: operational_event_detail,
            },
        ]
        .into_iter()
        .filter(|item| total > 1 || item.persisted_when_produced)
        .collect()
}

/// Describes transient processing and the boundaries of each non-retention claim.
pub(super) fn non_retention(
    total: usize,
    voice: usize,
    transcription: usize,
    provider_description: &str,
) -> Vec<PrivacyNonRetentionStatus> {
    let (audio_behavior, audio_boundary, interim_behavior, interim_boundary) = if total > 1 {
        (
            format!(
                "Across the {total} covered experiments, voice communication is enabled in {voice} and speech transcription is enabled in {transcription}. When voice is enabled, Parlando relays microphone audio live but does not store it."
            ),
            if transcription == 0 {
                "None of the covered experiments sends microphone audio to a transcription provider."
                    .to_string()
            } else {
                format!(
                    "Experiments with transcription enabled stream audio to {provider_description}; provider processing is governed by the institution's provider agreement and retention settings."
                )
            },
            if transcription == 0 {
                "None of the covered experiments generates or receives speech-recognition text."
                    .to_string()
            } else {
                "Experiments with transcription enabled store final transcript text but not changing interim recognition hypotheses."
                    .to_string()
            },
            "Final transcript text and timing are retained only for experiments with transcription enabled."
                .to_string(),
        )
    } else if voice == 0 {
        (
                "Voice communication is disabled. Parlando does not receive, relay, or store participant microphone audio for this experiment."
                    .to_string(),
                "No transcription provider receives microphone audio from Parlando."
                    .to_string(),
                "Speech transcription is disabled. Parlando does not generate or receive interim or final speech-recognition text for this experiment."
                    .to_string(),
                "Typed participant messages are unaffected and are stored as described above."
                    .to_string(),
            )
    } else if transcription == 0 {
        (
                "Voice communication is enabled. Parlando relays microphone audio live to the other participant but does not store it."
                    .to_string(),
                "Speech transcription is disabled, so Parlando does not send microphone audio to a transcription provider."
                    .to_string(),
                "Speech transcription is disabled. Parlando does not generate or receive speech-recognition text."
                    .to_string(),
                "Spoken audio is relayed live but is not converted to stored text by Parlando."
                    .to_string(),
            )
    } else {
        (
                format!(
                    "Voice communication and speech transcription are enabled. Parlando relays microphone audio live to the other participant and streams it to {provider_description}; Parlando does not store the audio."
                ),
                format!(
                    "{provider_description} processes the live stream under the institution's provider agreement and retention settings."
                ),
                "Parlando stores the final transcript returned for an utterance, not changing interim recognition hypotheses."
                    .to_string(),
                "Final transcript text and its timing are stored as described above."
                    .to_string(),
            )
    };
    let (browser_diagnostic_behavior, browser_diagnostic_boundary) = if total == 1 && voice == 0 {
        (
            "Voice communication is disabled, so the browser does not start a Parlando voice session. Parlando does not store browser voice-diagnostic reports."
                .to_string(),
            "No browser voice-diagnostic records are expected for this experiment."
                .to_string(),
        )
    } else if total == 1 {
        (
            "During an enabled voice session, the browser may report temporary audio problems to the diagnostics endpoint. The endpoint does not write those reports to SQLite."
                .to_string(),
            "This guarantee concerns browser diagnostic reports; status events produced by enabled server-side speech services are retained."
                .to_string(),
        )
    } else {
        (
            format!(
                "Voice communication is enabled in {voice} of {total} covered experiments. Parlando does not write browser voice-diagnostic reports to SQLite."
            ),
            "This guarantee concerns browser diagnostic reports; status events produced by enabled server-side speech services are retained."
                .to_string(),
            )
    };
    vec![
            PrivacyNonRetentionStatus {
                category: "Participant IP addresses".to_string(),
                behavior: "Parlando does not write participant network addresses to SQLite or the corpus export."
                    .to_string(),
                boundary: "A web server, hosting service, firewall, or other infrastructure outside Parlando may keep its own access logs; the institution must document and manage those separately."
                    .to_string(),
            },
            PrivacyNonRetentionStatus {
                category: "Raw microphone audio".to_string(),
                behavior: audio_behavior,
                boundary: audio_boundary,
            },
            PrivacyNonRetentionStatus {
                category: "Interim speech-recognition text".to_string(),
                behavior: interim_behavior,
                boundary: interim_boundary,
            },
            PrivacyNonRetentionStatus {
                category: "Browser voice diagnostics".to_string(),
                behavior: browser_diagnostic_behavior,
                boundary: browser_diagnostic_boundary,
            },
        ]
        .into_iter()
        .filter(|item| {
            total > 1
                || item.category == "Participant IP addresses"
                || (voice > 0
                    && (item.category == "Raw microphone audio"
                        || item.category == "Browser voice diagnostics"))
                || (transcription > 0 && item.category == "Interim speech-recognition text")
        })
        .collect()
}
