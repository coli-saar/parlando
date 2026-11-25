//! Loads the consent Markdown catalogue once per server process.
use crate::markdown_templates::{load_directory, MarkdownTemplate};
use anyhow::{bail, Result};
use std::{path::PathBuf, sync::OnceLock};

static CATALOGUE: OnceLock<Result<Vec<MarkdownTemplate>, String>> = OnceLock::new();

/// Reads the configured folder on first startup and freezes the snapshot until process restart.
/// Missing/unreadable folders and malformed or ambiguous templates prevent startup.
pub(crate) fn initialize() -> Result<&'static [MarkdownTemplate]> {
    let result = CATALOGUE.get_or_init(|| {
        let directory = std::env::var_os("PARLANDO_CONSENT_TEMPLATE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("templates/informed-consent")
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
    initialize().expect("consent template catalogue validated during server startup")
}

#[cfg(test)]
mod tests {
    use super::*;
    /// A fresh server process reads its selected folder once and keeps that snapshot.
    #[test]
    fn startup_snapshot_uses_environment_directory() {
        const CHILD: &str = "PARLANDO_TEST_CONSENT_SNAPSHOT";
        if std::env::var_os(CHILD).is_some() {
            let templates = initialize().unwrap();
            assert_eq!(templates.len(), 1);
            assert_eq!(templates[0].title, "Runtime template");
            let directory =
                PathBuf::from(std::env::var_os("PARLANDO_CONSENT_TEMPLATE_DIR").unwrap());
            std::fs::write(directory.join("later.md"), "# Later\nLate copy").unwrap();
            assert_eq!(catalogue().len(), 1);
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("runtime.md"),
            "# Runtime template\n\nLoaded from disk.",
        )
        .unwrap();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "consent_templates::tests::startup_snapshot_uses_environment_directory",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("PARLANDO_CONSENT_TEMPLATE_DIR", directory.path())
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
