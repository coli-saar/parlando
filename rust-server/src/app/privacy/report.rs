//! Builds the shared privacy report from the configurations selected by the route. It
//! combines contract versions, service disclosures, retention claims and the corpus
//! description. The renderers receive the completed report and need no database access.

use super::{
    configuration,
    exports::export_status,
    facts::PrivacyConfigFacts,
    retention::{non_retention, stored_data},
    PrivacyFeatureStatus, PrivacyOverviewStatus, PrivacyStatus,
};
use crate::storage::now_iso;

/// Projects the selected configurations into the single report used by all renderers.
/// Only declared contract versions contribute to the sorted, distinct version list.
/// External services are distinct by provider and purpose in first-seen order.
pub(super) fn build_report(
    facts: &[PrivacyConfigFacts],
    experiment_id: Option<&str>,
) -> PrivacyStatus {
    let total = facts.len();
    let mut contract_versions = facts
        .iter()
        .map(|fact| fact.contract_version.as_str())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    contract_versions.sort_unstable();
    contract_versions.dedup();
    let external_services = configuration::external_services(facts);
    let transcription = facts
        .iter()
        .filter(|fact| fact.transcription_enabled)
        .count();
    let voice = facts.iter().filter(|fact| fact.voice_enabled).count();
    let consent_items = facts.iter().map(|fact| fact.consent_items).sum::<usize>();
    let information_references = facts
        .iter()
        .filter(|fact| {
            !fact.participant_information_version.trim().is_empty()
                && !fact.participant_information_url.trim().is_empty()
        })
        .count();
    let mut transcription_providers = facts
        .iter()
        .filter(|fact| fact.transcription_enabled)
        .map(|fact| fact.transcription_provider.as_str())
        .filter(|provider| !provider.is_empty())
        .collect::<Vec<_>>();
    transcription_providers.sort_unstable();
    transcription_providers.dedup();
    let provider_description = if transcription_providers.is_empty() {
        "the configured transcription provider".to_string()
    } else {
        transcription_providers.join(", ")
    };
    let has_external_services = !external_services.is_empty();
    PrivacyStatus {
        generated_at: now_iso(),
        experiment_id: experiment_id.map(str::to_string),
        privacy_contract_version: if contract_versions.len() == 1 {
            contract_versions[0].to_string()
        } else if contract_versions.is_empty() {
            "Unspecified".to_string()
        } else {
            format!("Mixed ({})", contract_versions.join(", "))
        },
        overview: PrivacyOverviewStatus {
            purpose: if consent_items > 0 {
                "Run the experiment, reconnect participants to their sessions, let authorized experimenters monitor and analyze sessions, preserve configured consent evidence, and prepare the documented corpus export."
            } else {
                "Run the experiment, reconnect participants to their sessions, let authorized experimenters monitor and analyze sessions, and prepare the documented corpus export."
            }
            .to_string(),
            primary_storage: "The SQLite database file configured for this Parlando server. Parlando does not encrypt individual database fields itself; encryption of the disk and protection of database backups depend on the deployment."
                .to_string(),
            access: "Participants can use their own live session. Authenticated Parlando administrators can inspect experiment records, download the corpus, and delete participant data. A web server, hosting service, or other infrastructure in front of Parlando may have separate access and logging rules."
                .to_string(),
            retention: "Parlando does not automatically delete completed research sessions after a fixed time. Records remain in SQLite until an administrator deletes a participant and the participant's shared sessions, or the operator removes the database data. Copies in backups or external exports must be managed separately by the institution."
                .to_string(),
        },
        configuration: configuration::configuration(facts),
        storage: stored_data(facts, transcription, consent_items, &provider_description, has_external_services),
        not_retained: non_retention(total, voice, transcription, &provider_description),
        raw_audio_stored_by_parlando: false,
        external_services,
        exports: export_status(
            facts.iter().any(|fact| fact.human_vs_agent),
            voice < total,
            transcription > 0,
            facts.iter().any(|fact| fact.tts_enabled && fact.human_vs_agent),
            has_external_services,
        ),
        participant_deletion: PrivacyFeatureStatus {
            available: true,
            detail: "Deleting a human participant physically removes that participant and every session in which the participant appeared, including all messages, transcripts, actions, game states, operational events, role assignments, and consent records attached to those sessions. Because the complete shared session is removed, the other participant's contribution to that session is removed as well. Deletion is blocked while an affected session is still live or otherwise unfinished. Database backups and corpus files already copied elsewhere are not changed by this operation."
                .to_string(),
        },
        consent_evidence: PrivacyFeatureStatus {
            available: consent_items > 0,
            detail: if total == 1 {
                if consent_items == 0 {
                    format!(
                        "No consent items are configured for this experiment. A link and version for the participant-information document are {}.",
                        if information_references == 1 { "configured" } else { "not configured" }
                    )
                } else {
                    format!(
                        "For each configured consent item, Parlando records whether it was accepted, when it was declared, and a cryptographic fingerprint of the complete text shown. A link and version for the participant-information document are {}.",
                        if information_references == 1 { "configured" } else { "not configured" }
                    )
                }
            } else {
                format!(
                    "Across these {total} experiments, Parlando records whether each configured consent item was accepted, when it was declared, and a cryptographic fingerprint of the complete text shown. A link and version for the participant-information document are configured in {information_references} experiments."
                )
            },
        },
    }
}
