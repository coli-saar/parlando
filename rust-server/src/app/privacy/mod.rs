//! Privacy reports describe the configured collection, retention, and export of experiment data.
//!
//! The routes resolve the requested experiment scope into configuration facts. The report
//! builder turns those facts into one model for JSON, HTML and Markdown. Storage, speech
//! services and the corpus projector own the behavior described in the report.
use serde::Serialize;

mod configuration;
mod exports;
mod facts;
mod html;
mod markdown;
mod report;
mod retention;
mod routes;
#[cfg(test)]
mod tests;
pub(super) use routes::*;

#[derive(Clone, Debug, Serialize)]
pub(super) struct PrivacyStatus {
    generated_at: String,
    experiment_id: Option<String>,
    privacy_contract_version: String,
    overview: PrivacyOverviewStatus,
    configuration: Vec<PrivacyConfigurationStatus>,
    storage: Vec<PrivacyStorageStatus>,
    not_retained: Vec<PrivacyNonRetentionStatus>,
    raw_audio_stored_by_parlando: bool,
    external_services: Vec<PrivacyServiceStatus>,
    exports: PrivacyExportStatus,
    participant_deletion: PrivacyFeatureStatus,
    consent_evidence: PrivacyFeatureStatus,
}

/// One effective experiment setting that determines the report's processing claims.
#[derive(Clone, Debug, Serialize)]
struct PrivacyConfigurationStatus {
    setting: String,
    status: String,
    detail: String,
}

#[derive(Clone, Debug, Serialize)]
struct PrivacyOverviewStatus {
    purpose: String,
    primary_storage: String,
    access: String,
    retention: String,
}

#[derive(Clone, Debug, Serialize)]
struct PrivacyStorageStatus {
    category: String,
    persisted_when_produced: bool,
    purpose: String,
    detail: String,
}

#[derive(Clone, Debug, Serialize)]
struct PrivacyNonRetentionStatus {
    category: String,
    behavior: String,
    boundary: String,
}

#[derive(Clone, Debug, Serialize)]
struct PrivacyServiceStatus {
    service: String,
    enabled: bool,
    purpose: String,
    data_sent: String,
}

#[derive(Clone, Debug, Serialize)]
struct PrivacyExportStatus {
    available: bool,
    variant: String,
    schema_id: String,
    schema_sha256: String,
    scope: String,
    release_status: String,
    content_review_required: bool,
    formats: Vec<String>,
    identifiers: String,
    structure: String,
    timing: String,
    selection_rule: String,
    included_fields: Vec<PrivacyExportFieldStatus>,
    not_written: Vec<String>,
    detail: String,
}

#[derive(Clone, Debug, Serialize)]
struct PrivacyExportFieldStatus {
    section: String,
    description: String,
    fields: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
struct PrivacyFeatureStatus {
    available: bool,
    detail: String,
}

/// Formats a boolean fact consistently in HTML and Markdown reports.
fn yes_no(value: bool) -> &'static str {
    if value {
        "Yes"
    } else {
        "No"
    }
}
