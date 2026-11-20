//! Renders the shared privacy report for the administrator browser. Dynamic configuration
//! and provider text is escaped when placed into HTML. Decisions about collection and
//! retention come from the report builder.

use super::{yes_no, PrivacyStatus};

/// Renders the privacy status as a self-contained administrator page without client-side scripts.
pub(super) fn render_privacy_status_html(status: &PrivacyStatus) -> String {
    let services = if status.external_services.is_empty() {
        "<tr><td colspan=\"3\" class=\"empty\">No external speech services are enabled.</td></tr>"
            .to_string()
    } else {
        status
            .external_services
            .iter()
            .map(|service| {
                format!(
                    "<tr><td>{}</td><td>{}</td><td>{}</td></tr>",
                    escape_html_text(&service.service),
                    yes_no(service.enabled),
                    escape_html_text(&service.data_sent)
                )
            })
            .collect::<Vec<_>>()
            .join("")
    };
    let storage = status
        .storage
        .iter()
        .map(|item| {
            format!(
                "<tr><td>{}</td><td>{}</td><td>{}</td></tr>",
                escape_html_text(&item.category),
                yes_no(item.persisted_when_produced),
                escape_html_text(&item.detail)
            )
        })
        .collect::<Vec<_>>()
        .join("");
    let export_fields = status
        .exports
        .included_fields
        .iter()
        .map(|item| {
            format!(
                "<tr><td>{}</td><td>{}</td></tr>",
                escape_html_text(&item.section),
                escape_html_text(&item.fields.join(", "))
            )
        })
        .collect::<Vec<_>>()
        .join("");
    format!(
        r##"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Parlando Privacy Status</title>
  <style>
    :root {{ font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; background: #f6f7f9; color: #182026; }}
    * {{ box-sizing: border-box; }}
    body {{ margin: 0; min-height: 100vh; }}
    .app-header {{ align-items: center; background: #fff; border-bottom: 1px solid #dde2e7; display: flex; justify-content: space-between; min-height: 64px; padding: 12px 24px; }}
    .brand {{ display: grid; gap: 3px; }}
    .brand strong {{ font-size: 20px; }}
    .brand span {{ color: #65717c; font-size: 12px; font-weight: 700; text-transform: uppercase; }}
    .header-actions {{ display: flex; flex-wrap: wrap; gap: 8px; }}
    .button {{ background: #fff; border: 1px solid #c9d2da; border-radius: 6px; color: #31404c; font-size: 13px; font-weight: 750; padding: 8px 11px; text-decoration: none; }}
    .button.primary {{ background: #185f8f; border-color: #185f8f; color: #fff; }}
    main {{ margin: 0 auto; max-width: 1100px; padding: 28px 24px 48px; }}
    h1 {{ font-size: 28px; margin: 0 0 8px; }}
    h2 {{ font-size: 18px; margin: 0 0 12px; }}
    .intro {{ color: #53616c; margin: 0 0 20px; }}
    .grid {{ display: grid; gap: 14px; grid-template-columns: repeat(2, minmax(0, 1fr)); }}
    .panel {{ background: #fff; border: 1px solid #dce2e8; border-radius: 8px; margin-bottom: 14px; padding: 16px; }}
    .facts {{ display: grid; gap: 10px; grid-template-columns: repeat(2, minmax(0, 1fr)); }}
    .fact span {{ color: #65717c; display: block; font-size: 12px; margin-bottom: 3px; }}
    .fact strong {{ overflow-wrap: anywhere; }}
    table {{ border-collapse: collapse; font-size: 13px; width: 100%; }}
    th, td {{ border-bottom: 1px solid #e2e7eb; padding: 9px 8px; text-align: left; vertical-align: top; }}
    th {{ color: #52606b; font-size: 12px; }}
    .state {{ font-weight: 800; }}
    .empty {{ color: #65717c; font-size: 13px; }}
    @media (max-width: 760px) {{ .app-header {{ align-items: flex-start; padding: 10px 14px; }} .header-actions {{ justify-content: flex-end; }} main {{ padding: 20px 14px 36px; }} .grid, .facts {{ grid-template-columns: 1fr; }} table {{ display: block; overflow-x: auto; }} }}
  </style>
</head>
<body>
  <header class="app-header">
    <div class="brand"><strong>Parlando</strong><span>Privacy status</span></div>
    <nav class="header-actions"><a class="button" href="/admin/experiments">Dashboard</a><a class="button" download href="/api/admin/privacy.md">Markdown</a><a class="button primary" download href="/api/admin/privacy.json">JSON</a></nav>
  </header>
  <main>
    <h1>Privacy status</h1>
    <p class="intro">Installation-wide privacy behavior derived from the effective experiment configurations. This page does not infer controller identity, legal basis, retention decisions, hosting logs, or provider contracts.</p>
    <section class="panel">
      <h2>Scope</h2>
      <div class="facts"><div class="fact"><span>Generated</span><strong>{generated_at}</strong></div><div class="fact"><span>Privacy contract</span><strong>{contract_version}</strong></div></div>
    </section>
    <section class="panel">
      <h2>Storage behavior</h2>
      <table><thead><tr><th>Category</th><th>Stored by Parlando</th><th>Data retained</th></tr></thead><tbody>{storage}</tbody></table>
      <p class="state">Raw microphone audio stored by Parlando: {raw_audio}</p>
    </section>
    <section class="panel">
      <h2>External services</h2>
      <table><thead><tr><th>Service</th><th>Enabled</th><th>Data sent</th></tr></thead><tbody>{services}</tbody></table>
    </section>
    <div class="grid">
      <section class="panel"><h2>Corpus export</h2><p>Available: <strong>{export_available}</strong><br>Variant: <strong>{export_variant}</strong><br>Schema: <strong>{export_schema}</strong><br>Release status: <strong>{release_status}</strong><br>Formats: <strong>{export_formats}</strong></p><p>{export_scope}</p><p>{export_structure}</p><p>{export_timing}</p><p><strong>Field-selection rule:</strong> {selection_rule}</p><table><thead><tr><th>Output section</th><th>Fields written</th></tr></thead><tbody>{export_fields}</tbody></table><p class="empty">{export_detail}</p></section>
      <section class="panel"><h2>Participant administration</h2><p>Manual deletion available: <strong>{deletion}</strong></p><p class="empty">{deletion_detail}</p><p>Versioned information evidence available: <strong>{consent}</strong></p><p class="empty">{consent_detail}</p></section>
    </div>
  </main>
</body>
</html>"##,
        contract_version = escape_html_text(&status.privacy_contract_version),
        generated_at = escape_html_text(&status.generated_at),
        storage = storage,
        raw_audio = yes_no(status.raw_audio_stored_by_parlando),
        services = services,
        export_available = yes_no(status.exports.available),
        export_variant = escape_html_text(&status.exports.variant),
        export_schema = escape_html_text(&status.exports.schema_id),
        release_status = escape_html_text(&status.exports.release_status),
        export_formats = escape_html_text(&status.exports.formats.join(", ")),
        export_scope = escape_html_text(&status.exports.scope),
        export_structure = escape_html_text(&status.exports.structure),
        export_timing = escape_html_text(&status.exports.timing),
        selection_rule = escape_html_text(&status.exports.selection_rule),
        export_fields = export_fields,
        export_detail = escape_html_text(&status.exports.detail),
        deletion = yes_no(status.participant_deletion.available),
        deletion_detail = escape_html_text(&status.participant_deletion.detail),
        consent = yes_no(status.consent_evidence.available),
        consent_detail = escape_html_text(&status.consent_evidence.detail),
    )
}

/// Escapes untrusted configuration text before inserting it into server-rendered HTML.
fn escape_html_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
