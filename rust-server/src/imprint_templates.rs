//! Loads the imprint Markdown catalogue once per server process.
use crate::markdown_templates::{load_directory, MarkdownTemplate};
use anyhow::{bail, Result};
use std::{path::PathBuf, sync::OnceLock};

static CATALOGUE: OnceLock<Result<Vec<MarkdownTemplate>, String>> = OnceLock::new();

/// Reads the configured folder on first startup and freezes the snapshot until process restart.
/// Missing/unreadable folders and malformed or ambiguous templates prevent startup.
pub(crate) fn initialize() -> Result<&'static [MarkdownTemplate]> {
    let result = CATALOGUE.get_or_init(|| {
        let directory = std::env::var_os("PARLANDO_IMPRINT_TEMPLATE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("templates/imprint"));
        load_imprints(&directory).map_err(|error| format!("{error:#}"))
    });
    match result {
        Ok(templates) => Ok(templates),
        Err(error) => bail!("{error}"),
    }
}

/// Returns the validated startup snapshot for request handlers and section projections.
pub(crate) fn catalogue() -> &'static [MarkdownTemplate] {
    initialize().expect("imprint template catalogue validated during server startup")
}

/// Validates country identifiers used by the selector and saved imprint configuration.
fn load_imprints(directory: &std::path::Path) -> Result<Vec<MarkdownTemplate>> {
    let templates = load_directory(directory)?;
    for template in &templates {
        if template.id.len() != 2 || !template.id.bytes().all(|byte| byte.is_ascii_uppercase()) {
            bail!(
                "imprint template {:?} must have a two-letter uppercase country id (or EU)",
                template.id
            );
        }
    }
    Ok(templates)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Country identifiers come from metadata and are validated before exposing the selector.
    #[test]
    fn discovers_imprint_countries_and_rejects_invalid_codes() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("BE.md"),
            "---\nid: BE\ntitle: Belgium\n---\nOperator: {{operator|[name]}}",
        )
        .unwrap();
        let templates = load_imprints(directory.path()).unwrap();
        assert_eq!(templates[0].id, "BE");
        assert_eq!(templates[0].body, "Operator: {{operator|[name]}}");
        std::fs::write(
            directory.path().join("bad.md"),
            "---\nid: wrong\ntitle: Invalid\n---\nBody",
        )
        .unwrap();
        assert!(load_imprints(directory.path())
            .unwrap_err()
            .to_string()
            .contains("two-letter uppercase"));
    }

    /// A process reads the override directory once; later edits wait for a restart.
    #[test]
    fn startup_snapshot_uses_environment_directory() {
        const CHILD: &str = "PARLANDO_TEST_IMPRINT_SNAPSHOT";
        if std::env::var_os(CHILD).is_some() {
            assert_eq!(initialize().unwrap()[0].id, "BE");
            let directory =
                PathBuf::from(std::env::var_os("PARLANDO_IMPRINT_TEMPLATE_DIR").unwrap());
            std::fs::write(
                directory.join("FR.md"),
                "---\nid: FR\ntitle: France\n---\nNew copy",
            )
            .unwrap();
            assert_eq!(catalogue().len(), 1);
            let mut config = crate::config::ExperimentConfig::default();
            config.database.url = "sqlite:///:memory:".to_owned();
            config.direct.imprint.country = "BE".to_owned();
            config.validate().unwrap();
            config.direct.imprint.country = "DE".to_owned();
            assert!(config
                .validate()
                .unwrap_err()
                .to_string()
                .contains("unsupported imprint country"));
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("BE.md"),
            "---\nid: BE\ntitle: Belgium\n---\nRuntime notice.",
        )
        .unwrap();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "imprint_templates::tests::startup_snapshot_uses_environment_directory",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("PARLANDO_IMPRINT_TEMPLATE_DIR", directory.path())
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
}
