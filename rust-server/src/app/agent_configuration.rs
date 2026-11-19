//! Catalogue and validation of registered agent settings.

use super::{
    derived_agent_secret_key, AgentDefinition, AgentsMode, Context, ExperimentConfig, Result, Value,
};

/// Adds server-owned default secret references to serialized agent field descriptors.
pub(super) fn dashboard_agent_definitions(definitions: &[AgentDefinition]) -> Result<Value> {
    /// Adds dashboard secret-reference information to fields recursively.
    fn decorate(fields: &mut [Value], factory_id: &str, parent_path: &str) {
        for field in fields {
            let Some(object) = field.as_object_mut() else {
                continue;
            };
            let Some(key) = object
                .get("key")
                .and_then(Value::as_str)
                .map(str::to_string)
            else {
                continue;
            };
            let path = if parent_path.is_empty() {
                key
            } else {
                format!("{parent_path}.{key}")
            };
            match object.get("type").and_then(Value::as_str) {
                Some("secret_reference") => {
                    object.insert(
                        "secret_reference".to_string(),
                        Value::String(format!(
                            "game.{}",
                            derived_agent_secret_key(factory_id, &path)
                        )),
                    );
                }
                Some("object") => {
                    if let Some(children) = object.get_mut("fields").and_then(Value::as_array_mut) {
                        decorate(children, factory_id, &path);
                    }
                }
                _ => {}
            }
        }
    }

    let mut value = serde_json::to_value(definitions)?;
    for definition in value.as_array_mut().into_iter().flatten() {
        let Some(object) = definition.as_object_mut() else {
            continue;
        };
        let Some(factory_id) = object.get("id").and_then(Value::as_str).map(str::to_string) else {
            continue;
        };
        if let Some(fields) = object
            .get_mut("config_fields")
            .and_then(Value::as_array_mut)
        {
            decorate(fields, &factory_id, "");
        }
    }
    Ok(value)
}

/// Validates the selected factory settings and, when requested, referenced secret availability.
pub(super) fn validate_agent_configuration(
    definitions: &[AgentDefinition],
    config: &ExperimentConfig,
    require_secrets: bool,
) -> Result<()> {
    if config.agents.mode != AgentsMode::HumanVsAgent {
        return Ok(());
    }
    let selected = config
        .agents
        .human_vs_agent
        .as_ref()
        .context("human-versus-agent configuration is missing")?;
    let factory_id = selected
        .factory
        .as_deref()
        .context("no agent factory is selected")?;
    let definition = definitions
        .iter()
        .find(|definition| definition.id == factory_id)
        .with_context(|| format!("unknown agent factory {factory_id:?}"))?;
    let normalized = definition.normalize_settings(&selected.config)?;
    if require_secrets {
        definition.resolve_secrets(&normalized, &config.game_secrets)?;
    }
    Ok(())
}
