//! Audits and optionally redacts credential-shaped fields from stored experiment configuration.

use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use sqlx::{sqlite::SqliteConnectOptions, ConnectOptions, Row, SqlitePool};
use std::{env, str::FromStr};

/// Runs the explicit, dry-run-by-default database redaction utility.
#[tokio::main]
async fn main() -> Result<()> {
    let (database_url, apply) = parse_args()?;
    let options = SqliteConnectOptions::from_str(&database_url)
        .with_context(|| format!("invalid SQLite URL {database_url:?}"))?
        .disable_statement_logging();
    let pool = SqlitePool::connect_with(options).await?;
    let rows = sqlx::query("select experiment_id, config_json from experiments")
        .fetch_all(&pool)
        .await?;
    let mut changed = Vec::new();
    for row in rows {
        let experiment_id: String = row.try_get("experiment_id")?;
        let raw: String = row.try_get("config_json")?;
        let mut config: Value = serde_json::from_str(&raw)
            .with_context(|| format!("experiment {experiment_id} has malformed config_json"))?;
        let redactions = redact_secret_fields(&mut config);
        if redactions > 0 {
            changed.push((experiment_id, config, redactions));
        }
    }
    for (experiment_id, _, redactions) in &changed {
        println!("{experiment_id}: {redactions} credential-shaped field(s) require redaction");
    }
    if apply && !changed.is_empty() {
        let mut transaction = pool.begin().await?;
        for (experiment_id, config, _) in &changed {
            sqlx::query("update experiments set config_json = ? where experiment_id = ?")
                .bind(serde_json::to_string(config)?)
                .bind(experiment_id)
                .execute(&mut *transaction)
                .await?;
        }
        transaction.commit().await?;
        println!("redacted {} experiment row(s)", changed.len());
    } else if !apply {
        println!("dry run only; rerun with --apply after backing up the database");
    }
    Ok(())
}

/// Parses `DATABASE_URL [--apply]` without accepting ambiguous extra arguments.
fn parse_args() -> Result<(String, bool)> {
    let mut args = env::args().skip(1);
    let database_url = args
        .next()
        .ok_or_else(|| anyhow!("usage: redact_experiment_secrets <sqlite-url> [--apply]"))?;
    let mut apply = false;
    for argument in args {
        if argument == "--apply" && !apply {
            apply = true;
        } else {
            return Err(anyhow!("unexpected argument {argument:?}"));
        }
    }
    Ok((database_url, apply))
}

/// Recursively replaces known credential fields with null and returns the number changed.
fn redact_secret_fields(value: &mut Value) -> usize {
    match value {
        Value::Object(object) => object
            .iter_mut()
            .map(|(key, child)| {
                let normalized = key.to_ascii_lowercase();
                if is_secret_field(&normalized) && !child.is_null() {
                    *child = Value::Null;
                    1
                } else {
                    redact_secret_fields(child)
                }
            })
            .sum(),
        Value::Array(items) => items.iter_mut().map(redact_secret_fields).sum(),
        _ => 0,
    }
}

/// Recognizes exact and suffix-shaped credential field names used by runtime integrations.
fn is_secret_field(normalized: &str) -> bool {
    normalized == "private_key"
        || normalized == "client_secret"
        || normalized == "access_token"
        || normalized == "auth_token"
        || normalized.ends_with("_api_key")
        || normalized.ends_with("_password")
        || normalized.ends_with("_secret")
        || normalized.ends_with("_token")
        || matches!(
            normalized,
            "api_key" | "apikey" | "password" | "password_hash" | "token" | "secret"
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Confirms the migration is idempotent and preserves non-secret configuration.
    #[test]
    fn redaction_is_idempotent() {
        let mut config = serde_json::json!({
            "speechmatics": {"api_key": "sentinel", "model": "enhanced"},
            "nested": [{
                "token": "sentinel-2",
                "client_secret": "sentinel-3",
                "service_auth_token": "sentinel-4"
            }]
        });
        assert_eq!(redact_secret_fields(&mut config), 4);
        assert_eq!(redact_secret_fields(&mut config), 0);
        assert_eq!(config["speechmatics"]["model"], "enhanced");
        assert!(config["speechmatics"]["api_key"].is_null());
    }
}
