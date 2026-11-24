//! Public information sites belonging to a stored study, available before activation.
//! Only the explicit imprint text is published from configuration; other documents
//! remain static. Public reads and administrator downloads use the same renderer.

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

/// Renders only the explicitly public imprint text; no other configuration enters HTML.
fn information_site_html(audience: &str, template: &str, config: &Value) -> String {
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
    let mut response =
        Html(information_site_html(audience, html, &definition.config)).into_response();
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
        information_site_html(&page, html, &definition.config),
        "text/html; charset=utf-8",
        &format!("{experiment_id}-{page}.html"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Source URLs become safe links while non-HTTPS text remains inert.
    #[test]
    fn imprint_source_links_are_escaped() {
        let template = SITES.iter().find(|(name, _)| *name == "imprint").unwrap().1;
        let config = json!({"direct": {"imprint": {"text": "https://example.org/?a=\"x\"&b=1\njavascript:alert(1)"}}});
        let html = information_site_html("imprint", template, &config);
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
        let html = information_site_html("imprint", template, &config);
        assert!(html.contains("Institute &amp; team\n&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(!html.contains("<script>"));
        assert!(!html.contains("private-draft"));
        assert!(!html.contains("secret-value"));
        assert!(information_site_html("imprint", template, &json!({})).contains("<body></body>"));
    }
}
