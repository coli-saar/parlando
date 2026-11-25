//! Loads the privacy notice Markdown catalogue once per server process.
use crate::markdown_templates::{load_directory, MarkdownTemplate};
use anyhow::{bail, Result};
use std::{path::PathBuf, sync::OnceLock};

static CATALOGUE: OnceLock<Result<Vec<MarkdownTemplate>, String>> = OnceLock::new();

/// Reads the configured folder on first startup and freezes the snapshot until process restart.
/// Missing/unreadable folders and malformed or ambiguous templates prevent startup.
pub(crate) fn initialize() -> Result<&'static [MarkdownTemplate]> {
    let result = CATALOGUE.get_or_init(|| {
        let directory = std::env::var_os("PARLANDO_PRIVACY_NOTICE_TEMPLATE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("templates/privacy-notice")
            });
        load_directory(&directory).map_err(|error| format!("{error:#}"))
    });
    match result {
        Ok(templates) => Ok(templates),
        Err(error) => bail!("{error}"),
    }
}

/// Returns the validated startup snapshot for request handlers and section projections.
pub(crate) fn catalogue() -> &'static [MarkdownTemplate] {
    initialize().expect("privacy notice template catalogue validated during server startup")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every shipped jurisdiction survives configuration serialization with its editable notice text.
    #[test]
    fn six_jurisdictions_roundtrip_and_validate() {
        let ids: Vec<_> = catalogue()
            .iter()
            .map(|template| template.id.as_str())
            .collect();
        assert_eq!(ids, ["EU", "CA", "UK", "CH", "CAN", "BR"]);
        for id in ids {
            let mut config = crate::config::ExperimentConfig::default();
            config.database.url = "sqlite:///:memory:".to_owned();
            config.direct.privacy_notice.jurisdiction = id.to_owned();
            config.direct.privacy_notice.text = "Reviewed notice".to_owned();
            let value = serde_json::to_value(&config).unwrap();
            let mut decoded: crate::config::ExperimentConfig =
                serde_json::from_value(value).unwrap();
            // Installation-owned database settings are intentionally omitted from study serialization.
            decoded.database.url = config.database.url.clone();
            decoded.validate().unwrap();
            assert_eq!(decoded.direct.privacy_notice.text, "Reviewed notice");
        }
        let mut config = crate::config::ExperimentConfig::default();
        config.database.url = "sqlite:///:memory:".to_owned();
        config.direct.privacy_notice.jurisdiction = "invalid".to_owned();
        assert!(config
            .validate()
            .unwrap_err()
            .to_string()
            .contains("unsupported privacy notice"));
    }
}
