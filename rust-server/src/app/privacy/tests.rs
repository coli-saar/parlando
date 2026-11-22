use super::{facts::privacy_config_facts, report::build_report};
use serde_json::json;

/// A voice-only room relays microphone audio without generating transcript records.
#[test]
fn voice_relay_alone_adds_no_transcript_storage_or_external_service() {
    let facts = privacy_config_facts(&json!({"communication": {"mode": "speech"}}));
    let report = build_report(&[facts], Some("voice"));
    assert!(report.external_services.is_empty());
    assert!(report
        .not_retained
        .iter()
        .any(|item| item.category == "Raw microphone audio"));
    assert!(!report.storage.iter().any(|item| matches!(
        item.category.as_str(),
        "Final voice transcripts" | "Typed participant messages"
    )));
    assert!(report
        .exports
        .included_fields
        .iter()
        .any(|section| section.section == "Message event"
            && section.description.contains("produces no message events")));
}

/// Agent text remains in speech conversations even if recognition and synthesis are disabled.
#[test]
fn speech_agent_messages_are_reported_independently_of_transcription() {
    let facts = privacy_config_facts(&json!({
        "communication": {"mode": "speech"},
        "agents": {"mode": "human_vs_agent"}
    }));
    let report = build_report(&[facts], Some("speech-agent"));
    assert!(report
        .storage
        .iter()
        .any(|item| item.category == "Agent messages"));
    let message = report
        .exports
        .included_fields
        .iter()
        .find(|section| section.section == "Message event")
        .unwrap();
    assert!(message.description.contains("software-agent messages"));
    assert!(!message.description.contains("participant-typed messages"));
}

/// A provider used for two purposes appears once per data flow across all covered experiments.
#[test]
fn installation_report_deduplicates_services_by_provider_and_purpose() {
    let configuration = json!({
        "communication": {"mode": "speech"},
        "transcription": {"enabled": true, "provider": "speech"},
        "tts": {"enabled": true, "provider": "speech"}
    });
    let facts = [
        privacy_config_facts(&configuration),
        privacy_config_facts(&configuration),
    ];
    let report = build_report(&facts, None);
    assert_eq!(report.external_services.len(), 2);
    assert!(report
        .external_services
        .iter()
        .all(|item| item.service == "speech"));
    assert_ne!(
        report.external_services[0].data_sent,
        report.external_services[1].data_sent
    );
    assert_eq!(report.configuration[0].status, "2");
}

/// Only declared versions affect the installation summary, independent of catalogue order.
#[test]
fn report_distinguishes_unspecified_single_and_mixed_contract_versions() {
    let version = |v: &str| privacy_config_facts(&json!({"privacy": {"contract_version": v}}));
    let report = |versions: &[&str]| {
        let facts = versions.iter().map(|v| version(v)).collect::<Vec<_>>();
        build_report(&facts, None).privacy_contract_version
    };
    assert_eq!(report(&[""]), "Unspecified");
    assert_eq!(report(&["", "v2", "v2"]), "v2");
    assert_eq!(report(&["v2", "", "v1", "v2"]), "Mixed (v1, v2)");
}
