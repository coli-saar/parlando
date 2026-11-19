use super::{facts::privacy_config_facts, report::build_report};
use serde_json::json;

/// A voice-only room relays microphone audio without generating transcript records.
#[test]
fn voice_relay_alone_adds_no_transcript_storage_or_external_service() {
    let facts = privacy_config_facts(&json!({"voice": {"enabled": true}}));
    let report = build_report(&[facts], Some("voice"));
    assert!(report.external_services.is_empty());
    assert!(report
        .not_retained
        .iter()
        .any(|item| item.category == "Raw microphone audio"));
    assert!(!report
        .storage
        .iter()
        .any(|item| item.category == "Final voice transcripts"));
}

/// A provider used for two purposes appears once per data flow across all covered experiments.
#[test]
fn installation_report_deduplicates_services_by_provider_and_purpose() {
    let configuration = json!({
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
