//! Public information sites belonging to a stored study, available before activation.
//! Explicit imprint text and consent statements are published from configuration. Public reads and administrator downloads use the same renderer.

use super::*;
use axum::body::Body;

/// Maps each information-page identifier to its independently editable HTML document.
const SITES: &[(&str, &str)] = &[
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

/// Headings emitted by the consent builder; only complete heading lines delimit sections.
const CONSENT_SECTION_HEADINGS: &[&str] = &[
    "Subject of this study",
    "Duration and compensation",
    "Possible benefit of the study",
    "Experiences and risks",
    "Data processing",
    "Non-personal information collected",
    "Personal information collected",
    "Confidentiality",
    "Data storage",
    "Disclosure and publication",
    "Voluntary participation and withdrawal",
    "Access, correction, and deletion",
    "Data controller and contact person",
    "Declaration of consent",
    "Eligibility and study information",
    "Communication and privacy rules",
    "Live voice and transcription",
];

/// Sections omitted from study-conductor information; participant consent remains unchanged.
const CONDUCTOR_EXCLUDED_SECTIONS: &[&str] = &[
    "Subject of this study",
    "Possible benefit of the study",
    "Declaration of consent",
    "Communication and privacy rules",
];

/// Data protection information includes only these generated sections.
const DATA_PROTECTION_SECTIONS: &[&str] = &[
    "Personal information collected",
    "Non-personal information collected",
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
                && CONSENT_SECTION_HEADINGS.contains(&heading)
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

/// Renders explicitly public imprint and consent copy, excluding other configuration.
fn information_site_html(
    audience: &str,
    template: &str,
    config: &Value,
    consent_url: &str,
) -> String {
    if matches!(audience, "participants" | "conductors" | "data-protection") {
        let forms = config.pointer("/direct/consents").and_then(Value::as_array);
        let mut content = String::new();
        for form in forms.into_iter().flatten() {
            let body = form.get("body").and_then(Value::as_str).unwrap_or("");
            let title = form
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("Consent Form");
            let body = consent_body_for_audience(body, audience, title);
            if body.trim().is_empty() {
                continue;
            }
            if audience == "conductors" && CONDUCTOR_EXCLUDED_SECTIONS.contains(&title.trim()) {
                continue;
            }
            content.push_str(&format!("<section><h2>{}</h2><pre style=\"white-space:pre-wrap;overflow-wrap:anywhere;font:inherit\">{}</pre></section>", escape_document_text(title), escape_document_text(&body)));
        }
        if content.is_empty() {
            content.push_str("<p>Please configure the informed consent form.</p>");
        }
        let link = if audience == "data-protection" {
            format!(
                "<p><a href=\"{}\">View the full informed consent form</a></p>",
                escape_document_text(consent_url)
            )
        } else {
            String::new()
        };
        return template.replace(
            "{{content}}",
            &format!("<main><h1>Informed consent form</h1>{link}{content}</main>"),
        );
    }
    if audience != "imprint" {
        return template.to_owned();
    }
    let text = config
        .pointer("/direct/imprint/text")
        .and_then(Value::as_str)
        .unwrap_or("");
    if text.trim().is_empty() {
        return template.replace("{{content}}", "");
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
    template.replace("{{content}}", &format!("<main><h1>Imprint</h1><pre style=\"white-space:pre-wrap;overflow-wrap:anywhere;font:inherit\">{escaped}</pre></main>"))
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
    let Some((_, html)) = SITES.iter().find(|(name, _)| *name == audience) else {
        return Some(StatusCode::NOT_FOUND.into_response());
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
/// can be downloaded, and their bytes match the corresponding public page.
pub(super) async fn admin_information_site_download(
    State(context): State<Arc<ServerContext>>,
    Path((experiment_id, page)): Path<(String, String)>,
) -> Result<Response, AppError> {
    let (_, html) = SITES
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
        information_site_html(
            &page,
            html,
            &definition.config,
            &format!(
                "{}/information/participants/",
                experiment_public_base_url(&context.config.server.public_base_url, &experiment_id)
            ),
        ),
        "text/html; charset=utf-8",
        &format!("{experiment_id}-{page}.html"),
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
        let source = include_str!("admin_dashboard.js");
        let catalogue = source
            .split("const CONSENT_TEMPLATE_OPTIONS = ")
            .nth(1)
            .unwrap()
            .split("\n];")
            .next()
            .unwrap()
            .to_owned()
            + "\n]";
        let options: Vec<Value> = serde_json::from_str(&catalogue).unwrap();
        let headings: Vec<_> = options
            .iter()
            .map(|item| item["title"].as_str().unwrap())
            .collect();
        assert_eq!(headings, CONSENT_SECTION_HEADINGS);
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
