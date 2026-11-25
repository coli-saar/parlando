//! Public information sites belonging to a stored study, available before activation.
//! Explicit imprint text and consent statements are published from configuration. Public pages and Markdown downloads share audience-specific content selection.

use super::*;
use axum::body::Body;

/// Maps each information-page identifier to its independently editable HTML document.
const SITES: &[(&str, &str)] = &[
    ("privacy", include_str!("../../templates/study-sites/privacy.html")),
    (
        "imprint",
        include_str!("../../templates/study-sites/imprint.html"),
    ),
    (
        "participants",
        include_str!("../../templates/study-sites/participants.html"),
    ),
    (
        "data-protection",
        include_str!("../../templates/study-sites/data-protection.html"),
    ),
    (
        "conductors",
        include_str!("../../templates/study-sites/conductors.html"),
    ),
];

/// Publishes stable, deployment-aware URLs as soon as a study enters the catalogue.
pub(super) fn information_site_urls(base: &str) -> Value {
    Value::Object(
        SITES
            .iter()
            .map(|(audience, _)| {
                (
                    (*audience).to_string(),
                    json!(format!("{base}/information/{audience}/")),
                )
            })
            .collect(),
    )
}

/// Escapes conductor-authored plain text before including it in a public document.
fn escape_document_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Uses the startup catalogue for section boundaries, including newly discovered Markdown files.
fn consent_section_headings() -> Vec<&'static str> {
    crate::consent_templates::catalogue()
        .iter()
        .map(|template| template.title.as_str())
        .collect()
}

/// Sections omitted from study-conductor information; participant consent remains unchanged.
const CONDUCTOR_EXCLUDED_SECTIONS: &[&str] = &[
    "Subject of this study",
    "Possible benefit of the study",
    "Declaration of consent",
    "Communication and privacy rules",
];

/// Data protection information includes only these generated sections.
const DATA_PROTECTION_SECTIONS: &[&str] = &[
    "Data processing",
    "Personal information collected",
    "Pseudonymized research data collected",
    "Data controller and contact person",
    "Data storage",
    "Disclosure and publication",
];

/// Projects generated plain-text sections for an audience, retaining paragraphs and custom copy.
/// A recognized heading starts the body or follows a blank line. Edited headings are custom copy.
fn consent_body_for_audience(body: &str, audience: &str, title: &str) -> String {
    if !matches!(audience, "conductors" | "data-protection") {
        return body.to_owned();
    }
    let lines: Vec<_> = body.split('\n').collect();
    let mut keep =
        audience != "data-protection" || DATA_PROTECTION_SECTIONS.contains(&title.trim());
    lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| {
            let heading = line.trim();
            if (index == 0 || lines[index - 1].trim().is_empty())
                && consent_section_headings().contains(&heading)
            {
                keep = if audience == "data-protection" {
                    DATA_PROTECTION_SECTIONS.contains(&heading)
                } else {
                    !CONDUCTOR_EXCLUDED_SECTIONS.contains(&heading)
                };
            }
            keep.then_some(*line)
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}

/// Selects public consent copy once for both HTML pages and Markdown downloads.
fn consent_document_forms(audience: &str, config: &Value) -> Vec<(String, String)> {
    config
        .pointer("/direct/consents")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|form| {
            let title = form
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("Consent Form");
            if audience == "conductors" && CONDUCTOR_EXCLUDED_SECTIONS.contains(&title.trim()) {
                return None;
            }
            let body = consent_body_for_audience(
                form.get("body").and_then(Value::as_str).unwrap_or(""),
                audience,
                title,
            );
            (!body.trim().is_empty()).then(|| (title.to_owned(), body))
        })
        .collect()
}

/// One operational responsibility group, shared with dashboard previews via JSON.
#[derive(Deserialize)]
struct ConsentResponsibility {
    /// Catalogue identity used to keep each responsibility group unique.
    id: String,
    /// Human-readable group heading.
    title: String,
    /// Template headings which activate this group; empty means every configured form.
    sections: Vec<String>,
    /// Actions the study team must carry out, not actions performed by Parlando.
    tasks: Vec<String>,
}

/// Selects tasks from original participant copy, before conductor-specific exclusions.
/// Renamed or custom headings cannot be classified and receive the general review task.
fn conductor_responsibilities(config: &Value) -> Vec<&'static ConsentResponsibility> {
    static RULES: std::sync::OnceLock<Vec<ConsentResponsibility>> = std::sync::OnceLock::new();
    let rules = RULES.get_or_init(|| {
        serde_json::from_str(include_str!("consent_responsibilities.json"))
            .expect("valid embedded consent responsibilities")
    });
    let forms = consent_document_forms("participants", config);
    if forms.is_empty() {
        return Vec::new();
    }
    let mut sections = std::collections::HashSet::new();
    for (title, body) in forms {
        if consent_section_headings().contains(&title.trim()) {
            sections.insert(title.trim().to_owned());
        }
        let lines: Vec<_> = body.split('\n').collect();
        for (index, line) in lines.iter().enumerate() {
            if (index == 0 || lines[index - 1].trim().is_empty())
                && consent_section_headings().contains(&line.trim())
            {
                sections.insert(line.trim().to_owned());
            }
        }
    }
    rules
        .iter()
        .filter(|rule| {
            rule.sections.is_empty()
                || rule
                    .sections
                    .iter()
                    .any(|section| sections.contains(section))
        })
        .collect()
}

/// Renders informational tasks without interactive controls or implied completion tracking.
fn conductor_todo_html(config: &Value) -> String {
    let rules = conductor_responsibilities(config);
    if rules.is_empty() {
        return String::new();
    }
    let mut html = String::from("<section><h2>TODO for study conductors</h2><p>Responsibilities derived from the included template sections. Review additional commitments in edited text. This checklist does not perform or track the work.</p>");
    for rule in rules {
        html.push_str(&format!(
            "<section data-responsibility=\"{}\"><h3>{}</h3><ul>",
            escape_document_text(&rule.id),
            escape_document_text(&rule.title)
        ));
        for task in &rule.tasks {
            html.push_str(&format!("<li>{}</li>", escape_document_text(task)));
        }
        html.push_str("</ul></section>");
    }
    html.push_str("</section>");
    html
}

/// Emits portable, initially unchecked TODOs for study-conductor Markdown downloads.
fn conductor_todo_markdown(config: &Value) -> String {
    let rules = conductor_responsibilities(config);
    if rules.is_empty() {
        return String::new();
    }
    let mut markdown = String::from("## TODO for study conductors\n\nResponsibilities derived from the included template sections. Review additional commitments in edited text. This checklist does not perform or track the work.\n\n");
    for rule in rules {
        markdown.push_str(&format!("### {}\n\n", rule.title));
        for task in &rule.tasks {
            markdown.push_str(&format!("- [ ] {task}\n"));
        }
        markdown.push('\n');
    }
    markdown
}

/// Names the audience document consistently across public pages and downloads.
fn information_document_title(audience: &str) -> &'static str {
    match audience {
        "participants" => "Information for participants",
        "conductors" => "Information for study conductors",
        "data-protection" => "Information for data protection managers",
        "privacy" => "Privacy notice",
        _ => "Imprint",
    }
}

/// Retains meaningful custom titles while suppressing redundant generic consent headings.
fn show_consent_form_title(title: &str) -> bool {
    !title.trim().is_empty()
        && !matches!(
            title.trim().to_lowercase().as_str(),
            "consent form" | "informed consent form"
        )
}

/// Exports readable Markdown, retaining lists and converting generated section headings.
/// HTML-looking copy is escaped so a Markdown viewer does not interpret it as HTML.
fn information_site_markdown(audience: &str, config: &Value, consent_url: &str) -> String {
    if matches!(audience, "imprint" | "privacy") {
        let text = config
            .pointer(if audience == "privacy" { "/direct/privacy_notice/text" } else { "/direct/imprint/text" })
            .and_then(Value::as_str)
            .unwrap_or("");
        return format!("# {}\n\n{}\n", information_document_title(audience), escape_document_text(if audience == "privacy" && text.trim().is_empty() { "Please configure the privacy notice." } else { text }));
    }
    let mut markdown = format!("# {}\n\n", information_document_title(audience));
    if audience == "data-protection" {
        markdown.push_str(&format!(
            "[View information for participants](<{consent_url}>)\n\n"
        ));
    }
    let forms = consent_document_forms(audience, config);
    if forms.is_empty() {
        markdown.push_str("Please configure the informed consent form.\n");
    }
    for (title, body) in forms {
        let show_title = show_consent_form_title(&title);
        if show_title {
            let title = escape_document_text(&title).replace('\n', " ");
            markdown.push_str(&format!("## {title}\n\n"));
        }
        let lines: Vec<_> = body.split('\n').collect();
        for (index, line) in lines.iter().enumerate() {
            if (index == 0 || lines[index - 1].trim().is_empty())
                && consent_section_headings().contains(&line.trim())
            {
                markdown.push_str(if show_title { "### " } else { "## " });
            }
            markdown.push_str(&escape_document_text(line));
            markdown.push('\n');
        }
        markdown.push('\n');
    }
    if audience == "conductors" {
        markdown.push_str(&conductor_todo_markdown(config));
    }
    markdown
}

/// Enhances escaped plain-text documents with the same local renderer used in previews.
/// The external module obeys the existing CSP; without JavaScript the readable source remains.
fn markdown_document_shell(template: &str, consent_url: &str) -> String {
    let path = consent_url.parse::<http::Uri>().map(|uri| uri.path().to_owned()).unwrap_or_default();
    let base = path.strip_suffix("participants/").unwrap_or("../");
    let script_url = escape_document_text(&format!("{base}document-markdown.js"));
    template.replace("</body>", &format!("<script type=\"module\" src=\"{script_url}\"></script></body>"))
}

/// Renders explicitly public imprint and consent copy, excluding other configuration.
fn information_site_html(
    audience: &str,
    template: &str,
    config: &Value,
    consent_url: &str,
) -> String {
    let enhanced_template = if audience == "imprint" && config.pointer("/direct/imprint/text").and_then(Value::as_str).unwrap_or("").trim().is_empty() { template.to_owned() } else { markdown_document_shell(template, consent_url) };
    let template = enhanced_template.as_str();
    if matches!(audience, "participants" | "conductors" | "data-protection") {
        let mut content = String::new();
        for (title, body) in consent_document_forms(audience, config) {
            let heading = if show_consent_form_title(&title) {
                format!("<h2>{}</h2>", escape_document_text(&title))
            } else {
                String::new()
            };
            content.push_str(&format!("<section>{heading}<pre data-study-markdown style=\"white-space:pre-wrap;overflow-wrap:anywhere;font:inherit\">{}</pre></section>", escape_document_text(&body)));
        }
        if content.is_empty() {
            content.push_str("<p>Please configure the informed consent form.</p>");
        }
        if audience == "conductors" {
            content.push_str(&conductor_todo_html(config));
        }
        let link = if audience == "data-protection" {
            format!(
                "<p><a href=\"{}\">View information for participants</a></p>",
                escape_document_text(consent_url)
            )
        } else {
            String::new()
        };
        return template.replace(
            "{{content}}",
            &format!(
                "<main><h1>{}</h1>{link}{content}</main>",
                information_document_title(audience)
            ),
        );
    }
    if !matches!(audience, "imprint" | "privacy") {
        return template.to_owned();
    }
    let text = config
        .pointer(if audience == "privacy" { "/direct/privacy_notice/text" } else { "/direct/imprint/text" })
        .and_then(Value::as_str)
        .unwrap_or("");
    if text.trim().is_empty() {
        return template.replace("{{content}}", if audience == "privacy" { "<main><h1>Privacy notice</h1><p>Please configure the privacy notice.</p></main>" } else { "" });
    }
    let escaped = text
        .split('\n')
        .map(|line| {
            let escaped = line
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
                .replace('"', "&quot;");
            if line.starts_with("https://") && !line.chars().any(char::is_whitespace) {
                format!("<a href=\"{escaped}\" rel=\"noopener noreferrer\">{escaped}</a>")
            } else {
                escaped
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    template.replace("{{content}}", &format!("<main><h1>{}</h1><pre data-study-markdown style=\"white-space:pre-wrap;overflow-wrap:anywhere;font:inherit\">{escaped}</pre></main>", information_document_title(audience)))
}

/// Serves known sites for existing studies, without allocating a runtime.
/// Returns `None` for other participant paths so normal intake routing can continue.
pub(super) async fn serve_information_site(
    context: &ServerContext,
    experiment_id: &str,
    suffix: &str,
    method: &Method,
) -> Option<Response> {
    let audience = suffix.strip_prefix("/information/")?.trim_end_matches('/');
    let markdown_asset = audience == "document-markdown.js";
    let html = match SITES.iter().find(|(name, _)| *name == audience) {
        Some((_, html)) => *html,
        None if markdown_asset => "",
        None => return Some(StatusCode::NOT_FOUND.into_response()),
    };
    let definition = match context
        .installation
        .store
        .experiment_definition(experiment_id)
        .await
    {
        Ok(Some(definition)) => definition,
        Ok(None) => return Some(StatusCode::NOT_FOUND.into_response()),
        Err(error) => {
            tracing::error!(%error, "could not load study information site");
            return Some(StatusCode::INTERNAL_SERVER_ERROR.into_response());
        }
    };
    if method != Method::GET && method != Method::HEAD {
        return Some(
            (
                StatusCode::METHOD_NOT_ALLOWED,
                [(http::header::ALLOW, "GET, HEAD")],
            )
                .into_response(),
        );
    }
    if markdown_asset {
        let body = concat!(include_str!("document_markdown.mjs"), "\nenhanceDocumentMarkdown();\n");
        let mut response = ([(http::header::CONTENT_TYPE, "text/javascript; charset=utf-8"), (http::header::CACHE_CONTROL, "no-store")], body).into_response();
        if method == Method::HEAD { *response.body_mut() = Body::empty(); }
        return Some(response);
    }
    let mut response = Html(information_site_html(
        audience,
        html,
        &definition.config,
        &format!(
            "{}/information/participants/",
            experiment_public_base_url(&context.config.server.public_base_url, experiment_id)
        ),
    ))
    .into_response();
    response.headers_mut().insert(
        http::header::CACHE_CONTROL,
        HeaderValue::from_static("no-store"),
    );
    if method == Method::HEAD {
        *response.body_mut() = Body::empty();
    }
    Some(response)
}

/// Downloads a study document through the authenticated administrator origin.
/// The router supplies authentication; only registered pages of stored experiments
/// can be downloaded, using the same selected copy as the corresponding public page.
pub(super) async fn admin_information_site_download(
    State(context): State<Arc<ServerContext>>,
    Path((experiment_id, page)): Path<(String, String)>,
) -> Result<Response, AppError> {
    SITES
        .iter()
        .find(|(name, _)| *name == page)
        .ok_or_else(|| AppError::not_found("Information page not found."))?;
    let definition = context
        .installation
        .store
        .experiment_definition(&experiment_id)
        .await?
        .ok_or_else(|| AppError::not_found("Experiment not found."))?;
    Ok(attachment_response(
        information_site_markdown(
            &page,
            &definition.config,
            &format!(
                "{}/information/participants/",
                experiment_public_base_url(&context.config.server.public_base_url, &experiment_id)
            ),
        ),
        "text/markdown; charset=utf-8",
        &format!("{experiment_id}-{page}.md"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Audience documents preserve form order and escape copy without exposing private fields.
    #[test]
    fn audience_documents_render_public_consent_copy() {
        for audience in ["participants", "conductors"] {
            let template = SITES.iter().find(|(name, _)| *name == audience).unwrap().1;
            let config = json!({"direct": {"consents": [
            {"id": "private-id", "title": "First <form>", "body": "Line one\n<script>literal</script>", "required": true},
            {"title": "Second form", "body": "Second statement"}
        ]}, "secret": "private-value"});
            let html = information_site_html(
                audience,
                template,
                &config,
                "https://study.example/e/test/information/participants/",
            );
            assert!(html.contains("First &lt;form&gt;"));
            assert!(html.contains("Line one\n&lt;script&gt;literal&lt;/script&gt;"));
            assert!(html.find("First &lt;form&gt;").unwrap() < html.find("Second form").unwrap());
            assert!(!html.contains("private-"));
            assert!(!html.contains("<script>"));
            assert!(!html.contains("{{content}}"));
        }
    }

    /// Missing and blank forms both show the actionable configuration notice.
    #[test]
    fn audience_documents_without_statements_show_notice() {
        for audience in ["participants", "conductors", "data-protection"] {
            let template = SITES.iter().find(|(name, _)| *name == audience).unwrap().1;
            for config in [
                json!({}),
                json!({"direct": {"consents": []}}),
                json!({"direct": {"consents": [{"title": "Empty", "body": "  "}]}}),
            ] {
                let html = information_site_html(
                    audience,
                    template,
                    &config,
                    "https://study.example/e/test/information/participants/",
                );
                assert!(html.contains("Please configure the informed consent form."));
                assert!(!html.contains("{{content}}"));
            }
        }
    }

    /// Every generated heading is recognized, and conductor exclusions remove full sections.
    #[test]
    fn conductor_sections_match_builder_and_preserve_other_audiences() {
        let headings = consent_section_headings();
        let body = headings
            .iter()
            .map(|title| format!("{title}\nCopy for {title}.\n\nSecond paragraph for {title}."))
            .collect::<Vec<_>>()
            .join("\n\n");
        let config = json!({"direct": {"consents": [{"title": "Consent Form", "body": body}]}});
        for audience in ["participants", "conductors", "data-protection"] {
            let template = SITES.iter().find(|(name, _)| *name == audience).unwrap().1;
            let html = information_site_html(
                audience,
                template,
                &config,
                "https://study.example/e/test/information/participants/",
            );
            for title in &headings {
                let expected = if audience == "data-protection" {
                    DATA_PROTECTION_SECTIONS.contains(title)
                } else {
                    audience != "conductors" || !CONDUCTOR_EXCLUDED_SECTIONS.contains(title)
                };
                assert_eq!(html.contains(&format!("Copy for {title}.")), expected);
                assert_eq!(
                    html.contains(&format!("Second paragraph for {title}.")),
                    expected
                );
            }
        }
        assert_eq!(
            consent_body_for_audience("Custom copy.", "conductors", ""),
            "Custom copy."
        );
        let template = SITES
            .iter()
            .find(|(name, _)| *name == "conductors")
            .unwrap()
            .1;
        let config = json!({"direct": {"consents": [{"title": "Declaration of consent", "body": "I agree."}]}});
        let html = information_site_html(
            "conductors",
            template,
            &config,
            "https://study.example/e/test/information/participants/",
        );
        assert!(!html.contains("I agree."));
        assert!(html.contains("Please configure the informed consent form."));
    }

    /// Data protection supports individually titled sections and absolute links in downloaded HTML.
    #[test]
    fn data_protection_links_full_consent_and_limits_custom_copy() {
        let template = SITES
            .iter()
            .find(|(name, _)| *name == "data-protection")
            .unwrap()
            .1;
        let url = "https://study.example/e/test/information/participants/";
        let config = json!({"direct": {"consents": [
            {"title": "Data storage", "body": "Stored securely."},
            {"title": "Custom title", "body": "Unclassified copy."}
        ]}});
        let html = information_site_html("data-protection", template, &config, url);
        assert!(html.contains("Stored securely."));
        assert!(!html.contains("Unclassified copy."));
        assert!(html.contains(&format!("href=\"{url}\"")));
        let empty = information_site_html("data-protection", template, &json!({}), url);
        assert!(empty.contains("Please configure the informed consent form."));
        assert!(empty.contains(&format!("href=\"{url}\"")));
    }

    /// TODOs derive from original selected sections, deduplicate groups, and stay conductor-only.
    #[test]
    fn conductor_todos_follow_selected_responsibilities() {
        let config = json!({"direct": {"consents": [
            {"title": "Consent Form", "body": "Access, correction, and deletion\nContact the team.\n\nDeclaration of consent\nI agree."},
            {"title": "Data storage", "body": "Storage details."}
        ]}});
        let rules = conductor_responsibilities(&config);
        let ids: Vec<_> = rules.iter().map(|rule| rule.id.as_str()).collect();
        assert!(ids.contains(&"requests"));
        assert!(ids.contains(&"anonymization"));
        assert!(ids.contains(&"consent"));
        assert!(!ids.contains(&"voice"));
        assert_eq!(
            ids.len(),
            ids.iter().collect::<std::collections::HashSet<_>>().len()
        );
        let url = "https://study.example/information/participants/";
        for audience in ["participants", "conductors", "data-protection"] {
            let template = SITES.iter().find(|(name, _)| *name == audience).unwrap().1;
            let html = information_site_html(audience, template, &config, url);
            let markdown = information_site_markdown(audience, &config, url);
            assert_eq!(
                html.contains("TODO for study conductors"),
                audience == "conductors"
            );
            assert_eq!(
                markdown.contains("TODO for study conductors"),
                audience == "conductors"
            );
            if audience == "conductors" {
                assert!(html.contains("two weeks after participation"));
                assert!(markdown.contains("- [ ]"));
                assert!(!html.contains("<h2>Declaration of consent</h2>"));
            }
        }
        assert!(conductor_todo_html(&json!({})).is_empty());
        assert!(conductor_todo_markdown(&json!({"direct": {"consents": []}})).is_empty());
        let duration = json!({"direct": {"consents": [{"title": "Duration and compensation", "body": "Ten minutes."}]}});
        assert!(!conductor_todo_html(&duration).contains("Two-week anonymization"));
        let custom =
            json!({"direct": {"consents": [{"title": "Custom", "body": "Custom commitment."}]}});
        assert_eq!(
            conductor_responsibilities(&custom)
                .iter()
                .map(|rule| rule.id.as_str())
                .collect::<Vec<_>>(),
            vec!["review"]
        );
        let catalogue: Vec<ConsentResponsibility> =
            serde_json::from_str(include_str!("consent_responsibilities.json")).unwrap();
        for heading in consent_section_headings() {
            assert!(
                catalogue
                    .iter()
                    .any(|rule| rule.sections.iter().any(|section| section == heading)),
                "missing duties for {heading}"
            );
        }
    }

    /// Markdown retains lists, headings and links while honoring every audience projection.
    #[test]
    fn markdown_downloads_preserve_content_and_audience_selection() {
        let config = json!({"direct": {"consents": [{"title": "Consent Form",
            "body": "Subject of this study\nStudy purpose.\n\nPersonal information collected\n- Contact information\n- <script>literal</script>\n\nData storage\nSecure storage."}]}});
        let url = "https://study.example/e/test/information/participants/";
        for audience in ["participants", "conductors", "data-protection"] {
            let markdown = information_site_markdown(audience, &config, url);
            assert!(
                markdown.starts_with(&format!("# {}\n\n", information_document_title(audience)))
            );
            assert!(!markdown.contains("## Consent Form"));
            assert!(!markdown.contains("# Informed consent form"));
            let template = SITES.iter().find(|(name, _)| *name == audience).unwrap().1;
            let html = information_site_html(audience, template, &config, url);
            assert!(html.contains(&format!(
                "<h1>{}</h1>",
                information_document_title(audience)
            )));
            assert!(!html.contains("<h2>Consent Form</h2>"));
            assert!(markdown.contains("## Personal information collected\n- Contact information"));
            assert!(markdown.contains("- &lt;script&gt;literal&lt;/script&gt;"));
            assert!(markdown.contains("## Data storage\nSecure storage."));
            assert_eq!(
                markdown.contains("Study purpose."),
                audience == "participants"
            );
            assert_eq!(
                markdown.contains(&format!("[View information for participants](<{url}>)")),
                audience == "data-protection"
            );
            assert!(information_site_markdown(audience, &json!({}), url)
                .contains("Please configure the informed consent form."));
        }
        let imprint =
            json!({"direct": {"imprint": {"text": "Institute\nhttps://example.org/legal"}}});
        assert_eq!(
            information_site_markdown("imprint", &imprint, url),
            "# Imprint\n\nInstitute\nhttps://example.org/legal\n"
        );
    }

    /// The privacy page and download publish only the saved snapshot, never private draft fields.
    #[test]
    fn privacy_notice_publishes_saved_text_and_empty_instruction() {
        let template = SITES.iter().find(|(name, _)| *name == "privacy").unwrap().1;
        let config = json!({"direct": {"privacy_notice": {
            "jurisdiction": "EU", "text": "Notice <script>alert(1)</script>\nhttps://example.org/law",
            "details": {"hidden": "UNPUBLISHED_DETAIL"}
        }}, "secret": "PRIVATE_VALUE"});
        let html = information_site_html("privacy", template, &config, "");
        assert!(html.contains("<h1>Privacy notice</h1>"));
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("UNPUBLISHED_DETAIL"));
        assert!(!html.contains("PRIVATE_VALUE"));
        let markdown = information_site_markdown("privacy", &config, "");
        assert!(markdown.starts_with("# Privacy notice\n\nNotice &lt;script&gt;"));
        assert!(markdown.contains("https://example.org/law"));
        for text in [information_site_html("privacy", template, &json!({}), ""), information_site_markdown("privacy", &json!({}), "")] {
            assert!(text.contains("Please configure the privacy notice."));
        }
    }

    /// Script URLs remain same-origin and retain deployment/game prefixes, even via a host alias.
    #[test]
    fn markdown_document_shell_preserves_hosted_paths() {
        let html = markdown_document_shell("<body>text</body>", "https://public.example/deployment/e/game~study/information/participants/");
        assert!(html.contains("src=\"/deployment/e/game~study/information/document-markdown.js\""));
        assert!(!html.contains("https://public.example"));
        assert!(html.contains("type=\"module\""));
    }

    /// Source URLs become safe links while non-HTTPS text remains inert.
    #[test]
    fn imprint_source_links_are_escaped() {
        let template = SITES.iter().find(|(name, _)| *name == "imprint").unwrap().1;
        let config = json!({"direct": {"imprint": {"text": "https://example.org/?a=\"x\"&b=1\njavascript:alert(1)"}}});
        let html = information_site_html(
            "imprint",
            template,
            &config,
            "https://study.example/e/test/information/participants/",
        );
        assert!(html.contains("href=\"https://example.org/?a=&quot;x&quot;&amp;b=1\""));
        assert!(!html.contains("href=\"javascript:"));
    }

    /// Public HTML exposes only reviewed copy and treats HTML-looking text as literal text.
    #[test]
    fn imprint_renders_only_escaped_public_text() {
        let template = SITES.iter().find(|(name, _)| *name == "imprint").unwrap().1;
        let config = json!({"direct": {"imprint": {
            "text": "Institute & team\n<script>alert(1)</script>",
            "email": "private-draft@example.org"
        }}, "private": "secret-value"});
        let html = information_site_html(
            "imprint",
            template,
            &config,
            "https://study.example/e/test/information/participants/",
        );
        assert!(html.contains("Institute &amp; team\n&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(!html.contains("<script>"));
        assert!(!html.contains("private-draft"));
        assert!(!html.contains("secret-value"));
        assert!(information_site_html(
            "imprint",
            template,
            &json!({}),
            "https://study.example/e/test/information/participants/"
        )
        .contains("<body></body>"));
    }
}
