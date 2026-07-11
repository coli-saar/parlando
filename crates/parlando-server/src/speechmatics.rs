use anyhow::{bail, Context, Result};
use serde_json::json;

use crate::config::SpeechmaticsConfig;

pub async fn create_speechmatics_temporary_key(config: &SpeechmaticsConfig) -> Result<String> {
    let response: serde_json::Value = reqwest::Client::new()
        .post(format!("{}?type=rt", config.management_url))
        .bearer_auth(&config.api_key)
        .json(&json!({ "ttl": config.temporary_key_ttl_seconds }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let key = response
        .get("key_value")
        .and_then(|value| value.as_str())
        .context("Speechmatics did not return key_value")?;
    if key.is_empty() {
        bail!("Speechmatics returned an empty temporary key");
    }
    Ok(key.to_string())
}
