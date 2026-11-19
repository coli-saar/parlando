use super::PrivacyStatus;

/// Renders a portable experiment data-processing record for archival with collected data.
pub(super) fn render_privacy_status_markdown(status: &PrivacyStatus) -> String {
    let subject = status.experiment_id.as_deref().map_or_else(
        || "all experiments".to_string(),
        |id| format!("experiment {id}"),
    );
    let mut output = format!(
        "# Data processing record — {}\n\nThis record describes how Parlando stored, processed, exported, and deleted participant data for {}. It was generated for retention with the experiment data. The data-handling contract version identifies the Parlando storage, export, and deletion behavior documented here.\n\n| Record | Value |\n| --- | --- |\n| Generated | {} |\n| Parlando data-handling contract version | {} |\n\n## Scope and responsibility\n\n| Topic | Description |\n| --- | --- |\n| Purpose | {} |\n| Primary storage | {} |\n| Access through Parlando | {} |\n| Retention | {} |\n\n## Privacy-relevant experiment settings\n\n| Setting | Status | Effect on data processing |\n| --- | --- | --- |\n",
        markdown_cell(&subject),
        markdown_cell(&subject),
        markdown_cell(&status.generated_at),
        markdown_cell(&status.privacy_contract_version),
        markdown_cell(&status.overview.purpose),
        markdown_cell(&status.overview.primary_storage),
        markdown_cell(&status.overview.access),
        markdown_cell(&status.overview.retention),
    );
    for item in &status.configuration {
        output.push_str(&format!(
            "| {} | {} | {} |\n",
            markdown_cell(&item.setting),
            markdown_cell(&item.status),
            markdown_cell(&item.detail)
        ));
    }
    output.push_str("\n## Data retained in Parlando's SQLite database\n\n| Information | Why it is processed | What is retained |\n| --- | --- | --- |\n");
    for item in &status.storage {
        output.push_str(&format!(
            "| {} | {} | {} |\n",
            markdown_cell(&item.category),
            markdown_cell(&item.purpose),
            markdown_cell(&item.detail)
        ));
    }
    output.push_str("\n## Data not retained by Parlando\n\n| Information | Parlando guarantee | Boundary of the guarantee |\n| --- | --- | --- |\n");
    for item in &status.not_retained {
        output.push_str(&format!(
            "| {} | {} | {} |\n",
            markdown_cell(&item.category),
            markdown_cell(&item.behavior),
            markdown_cell(&item.boundary)
        ));
    }
    output.push_str(
        "\n## External speech services\n\n| Service | Purpose | Data sent |\n| --- | --- | --- |\n",
    );
    if status.external_services.is_empty() {
        output.push_str("| None | Not applicable | No external speech service is enabled for this experiment. |\n");
    } else {
        for service in &status.external_services {
            output.push_str(&format!(
                "| {} | {} | {} |\n",
                markdown_cell(&service.service),
                markdown_cell(&service.purpose),
                markdown_cell(&service.data_sent)
            ));
        }
    }
    output.push_str(&format!(
        "\n## Corpus export\n\n{} {}\n\n{} {}\n\n**How fields are selected:** {}\n\n| Part of the corpus | What it contains |\n| --- | --- |\n",
        markdown_cell(&status.exports.structure),
        markdown_cell(&status.exports.scope),
        markdown_cell(&status.exports.identifiers),
        markdown_cell(&status.exports.timing),
        markdown_cell(&status.exports.selection_rule),
    ));
    for item in &status.exports.included_fields {
        output.push_str(&format!(
            "| {} | {} |\n",
            markdown_cell(&item.section),
            markdown_cell(&item.description)
        ));
    }
    output.push_str("\n**Stored or used by Parlando but not written to the corpus:**\n\n");
    for item in &status.exports.not_written {
        output.push_str(&format!("- {}\n", markdown_cell(item)));
    }
    let institution_provider_addition = if status.external_services.is_empty() {
        String::new()
    } else {
        ", and agreements and retention settings for the external speech services listed above"
            .to_string()
    };
    output.push_str(&format!(
        "\n**Before sharing:** {}\n\nThe export uses schema `{}` and is available as {}.\n\n## Consent evidence and deletion\n\n| Area | Behavior |\n| --- | --- |\n| Consent evidence | {} |\n| Participant deletion | {} |\n\n## What the institution must add\n\nThis record covers behavior enforced by Parlando. The institution must keep it together with its own record of the data controller, legal basis, retention schedule, server location, transport and disk encryption, backup policy, administrator access, and reverse-proxy or hosting logs{}.\n",
        markdown_cell(&status.exports.detail),
        markdown_cell(&status.exports.schema_id),
        status.exports.formats.iter().map(|value| markdown_cell(value)).collect::<Vec<_>>().join(", "),
        markdown_cell(&status.consent_evidence.detail),
        markdown_cell(&status.participant_deletion.detail),
        institution_provider_addition,
    ));
    output
}

/// Escapes one value for safe use inside a compact Markdown table cell or list item.
fn markdown_cell(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('|', "\\|")
        .replace(['\r', '\n'], " ")
}
