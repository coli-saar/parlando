use anyhow::{bail, Context, Result};
use serde_json::json;

use crate::config::SpeechmaticsConfig;

/// Mints a temporary realtime Speechmatics key through the management API.
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

#[cfg(test)]
mod tests {
    use axum::{extract::Query, http::HeaderMap, routing::post, Json, Router};
    use serde_json::{json, Value};
    use tokio::net::TcpListener;

    use super::*;

    // Starts a local Speechmatics-like management endpoint for temporary-key tests.
    async fn spawn_mock_speechmatics() -> String {
        async fn handler(
            Query(query): Query<std::collections::HashMap<String, String>>,
            headers: HeaderMap,
            Json(body): Json<Value>,
        ) -> Json<Value> {
            assert_eq!(query.get("type").map(String::as_str), Some("rt"));
            assert_eq!(
                headers
                    .get(axum::http::header::AUTHORIZATION)
                    .and_then(|value| value.to_str().ok()),
                Some("Bearer permanent-key")
            );
            assert_eq!(body["ttl"], 123);
            Json(json!({"key_value": "temporary-key"}))
        }

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let router = Router::new().route("/", post(handler));
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn speechmatics_temporary_key_uses_management_api_response() {
        let management_url = spawn_mock_speechmatics().await;
        let config = SpeechmaticsConfig {
            enabled: true,
            api_key: "permanent-key".to_string(),
            temporary_key_ttl_seconds: 123,
            management_url,
            ..SpeechmaticsConfig::default()
        };

        let key = create_speechmatics_temporary_key(&config).await.unwrap();

        assert_eq!(key, "temporary-key");
    }

    #[tokio::test]
    async fn speechmatics_temporary_key_rejects_missing_key_value() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let router = Router::new().route("/", post(|| async { Json(json!({})) }));
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let config = SpeechmaticsConfig {
            enabled: true,
            api_key: "permanent-key".to_string(),
            management_url: format!("http://{addr}"),
            ..SpeechmaticsConfig::default()
        };

        assert!(create_speechmatics_temporary_key(&config)
            .await
            .unwrap_err()
            .to_string()
            .contains("key_value"));
    }
}
