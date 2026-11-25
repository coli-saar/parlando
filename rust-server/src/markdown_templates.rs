//! Reads ordered Markdown template catalogues with optional YAML front matter.
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

/// One selectable template section; its body remains editable plain text.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct MarkdownTemplate {
    /// Stable identity used by the checkbox and draft text field.
    pub id: String,
    /// Heading emitted into generated consent statements.
    pub title: String,
    /// Brief explanation displayed beside the checkbox.
    pub description: String,
    /// Markdown source supplied as the editable section default.
    pub body: String,
    /// Explicit ordering takes precedence over the relative filename.
    #[serde(skip)]
    order: u32,
    /// Relative filename provides deterministic ordering for equal priorities.
    #[serde(skip)]
    source: PathBuf,
}

/// Optional YAML front matter; files without it derive metadata from their filename/content.
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Metadata {
    id: Option<String>,
    title: Option<String>,
    description: Option<String>,
    order: Option<u32>,
}

/// Enumerates Markdown files recursively without following symlinked directory trees.
fn markdown_files(directory: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(directory)
        .with_context(|| format!("read Markdown template directory {}", directory.display()))?
    {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            markdown_files(&entry.path(), files)?;
        } else if kind.is_file()
            && entry
                .path()
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
        {
            files.push(entry.path());
        }
    }
    Ok(())
}

/// Builds a deterministic catalogue from all Markdown files and rejects duplicate identities/headings.
pub(crate) fn load_directory(directory: &Path) -> Result<Vec<MarkdownTemplate>> {
    let mut paths = Vec::new();
    markdown_files(directory, &mut paths)?;
    paths.sort();
    let mut ids = HashSet::new();
    let mut titles = HashSet::new();
    let mut templates = Vec::new();
    for path in paths {
        let source = std::fs::read_to_string(&path)
            .with_context(|| format!("read Markdown template {}", path.display()))?;
        let relative = path.strip_prefix(directory)?;
        let template = parse_template(relative, &source)
            .with_context(|| format!("parse Markdown template {}", path.display()))?;
        if !ids.insert(template.id.clone()) {
            bail!(
                "duplicate Markdown template id {:?} in {}",
                template.id,
                path.display()
            );
        }
        if !titles.insert(template.title.clone()) {
            bail!(
                "duplicate Markdown template title {:?} in {}",
                template.title,
                path.display()
            );
        }
        templates.push(template);
    }
    templates.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.source.cmp(&b.source)));
    Ok(templates)
}

/// Separates optional front matter from the body and derives useful labels for plain Markdown.
/// A leading H1 supplies the title when no metadata title exists and is omitted from the body.
fn parse_template(relative: &Path, source: &str) -> Result<MarkdownTemplate> {
    let normalized = source.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let (metadata, body) = if let Some(rest) = normalized.strip_prefix("---\n") {
        let mut offset = 0;
        let mut end = None;
        for line in rest.split_inclusive('\n') {
            if line.trim_end_matches('\n') == "---" {
                end = Some((offset, offset + line.len()));
                break;
            }
            offset += line.len();
        }
        let (header_end, body_start) = end.context("missing closing --- for YAML front matter")?;
        (
            serde_yaml::from_str::<Metadata>(&rest[..header_end])?,
            &rest[body_start..],
        )
    } else {
        (Metadata::default(), normalized.as_str())
    };
    let mut body = body.trim().to_owned();
    let leading_title = body
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("# "))
        .map(str::to_owned);
    let title = match metadata.title {
        Some(title) => title.trim().to_owned(),
        None => match leading_title {
            Some(title) => {
                body = body
                    .split_once('\n')
                    .map_or("", |(_, rest)| rest)
                    .trim()
                    .to_owned();
                title.trim().to_owned()
            }
            None => relative
                .file_stem()
                .context("template filename has no stem")?
                .to_string_lossy()
                .replace(['-', '_'], " "),
        },
    };
    let id = metadata.id.unwrap_or_else(|| {
        relative
            .with_extension("")
            .to_string_lossy()
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                    ch
                } else {
                    '-'
                }
            })
            .collect()
    });
    if id.is_empty()
        || !id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        bail!("id must use ASCII letters, digits, hyphens, underscores, or dots");
    }
    if title.is_empty() || title.contains(['\n', '\r']) {
        bail!("title must be a nonempty single line");
    }
    let description = metadata
        .description
        .unwrap_or_else(|| format!("Include the {title} section."));
    if description.trim().is_empty() {
        bail!("description must not be empty");
    }
    Ok(MarkdownTemplate {
        id,
        title,
        description,
        body,
        order: metadata.order.unwrap_or(1000),
        source: relative.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Disk discovery includes new files and subfolders, honors metadata, and preserves body text.
    #[test]
    fn discovers_markdown_and_orders_templates() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("extra")).unwrap();
        std::fs::write(
            dir.path().join("plain.md"),
            "# Plain section\n\nMy **editable** copy.",
        )
        .unwrap();
        std::fs::write(dir.path().join("extra/first.MD"), "---\nid: stable_id\ntitle: First\ndescription: A description\norder: 1\n---\n\n{{CONTACT}}\n").unwrap();
        std::fs::write(dir.path().join("ignored.txt"), "Ignored").unwrap();
        let templates = load_directory(dir.path()).unwrap();
        assert_eq!(templates.len(), 2);
        assert_eq!(templates[0].id, "stable_id");
        assert_eq!(templates[0].body, "{{CONTACT}}");
        assert_eq!(templates[1].title, "Plain section");
        assert_eq!(templates[1].body, "My **editable** copy.");
        std::fs::write(dir.path().join("new.md"), "New copy.").unwrap();
        assert_eq!(load_directory(dir.path()).unwrap().len(), 3);
    }

    /// Invalid catalogues fail clearly instead of silently dropping selectable sections.
    #[test]
    fn rejects_invalid_and_duplicate_metadata() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_directory(dir.path()).unwrap().is_empty());
        let plain = parse_template(Path::new("new section.md"), "# New section\n\nBody.").unwrap();
        assert_eq!(plain.id, "new-section");
        assert!(load_directory(&dir.path().join("missing")).is_err());
        assert!(parse_template(Path::new("bad.md"), "---\ntitle: Broken").is_err());
        assert!(parse_template(Path::new("bad.md"), "---\nunknown: true\n---\nText").is_err());
        std::fs::write(dir.path().join("a.md"), "---\nid: same\ntitle: A\n---\nA").unwrap();
        std::fs::write(dir.path().join("b.md"), "---\nid: same\ntitle: B\n---\nB").unwrap();
        assert!(load_directory(dir.path())
            .unwrap_err()
            .to_string()
            .contains("duplicate Markdown template id"));
        std::fs::write(dir.path().join("b.md"), "---\nid: other\ntitle: A\n---\nB").unwrap();
        assert!(load_directory(dir.path())
            .unwrap_err()
            .to_string()
            .contains("duplicate Markdown template title"));
    }
}
