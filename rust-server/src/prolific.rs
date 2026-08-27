//! Narrow server-only client for Prolific study, submission, and signed-launch facts.

use std::{collections::HashMap, sync::Arc, time::Duration};

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Utc};
use jsonwebtoken::{decode, decode_header, jwk::JwkSet, Algorithm, DecodingKey, Validation};
use reqwest::{Client, StatusCode};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tokio::sync::RwLock;

const PROLIFIC_ISSUER: &str = "https://www.prolific.com";
const JWKS_TTL_SECONDS: i64 = 24 * 60 * 60;

/// One action attached to a Prolific completion path.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct CompletionAction {
    pub action: String,
}

/// One researcher-configured Prolific completion path.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct CompletionCode {
    pub code: Option<String>,
    pub code_type: String,
    #[serde(default)]
    pub actions: Vec<CompletionAction>,
}

/// Study fields required for Parlando activation and signed launch verification.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Study {
    pub id: String,
    pub external_study_url: String,
    pub prolific_id_option: String,
    #[serde(default)]
    pub completion_codes: Vec<CompletionCode>,
    pub estimated_completion_time: i64,
    pub maximum_allowed_time: Option<f64>,
    #[serde(default)]
    pub is_external_study_url_secure: bool,
    #[serde(default)]
    pub device_compatibility: Vec<String>,
    #[serde(default)]
    pub peripheral_requirements: Vec<String>,
    pub project: Option<String>,
    pub status: Option<String>,
}

/// Submission facts retained by Parlando; reward and bonus fields are deliberately absent.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Submission {
    pub id: String,
    pub participant: Option<String>,
    pub study_id: String,
    pub status: String,
    pub entered_code: Option<String>,
    pub return_requested: Option<String>,
}

/// Workspace description shown after a protected connection is verified.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Workspace {
    pub id: String,
    pub title: String,
}

/// Project ownership needed to bind a study to the configured workspace.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Project {
    pub id: String,
    pub workspace: String,
}

/// Launch identifiers extracted only after a signed token is verified.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedLaunch {
    pub participant_id: String,
    pub study_id: String,
    pub session_id: String,
    pub workspace_id: String,
    pub organisation_id: Option<String>,
}

/// Expected values which bind a signed launch to one configured experiment.
pub struct SignedLaunchExpectation<'a> {
    pub audience: &'a str,
    pub study_id: &'a str,
    pub workspace_id: &'a str,
}

#[derive(Clone, Debug, Deserialize)]
struct SignedLaunchClaims {
    sub: String,
    prolific: SignedLaunchPayload,
}

#[derive(Clone, Debug, Deserialize)]
struct SignedLaunchPayload {
    #[serde(rename = "PROLIFIC_PID")]
    participant_id: String,
    #[serde(rename = "STUDY_ID")]
    study_id: String,
    #[serde(rename = "SESSION_ID")]
    session_id: String,
    workspace_id: String,
    organisation_id: Option<String>,
}

#[derive(Clone)]
struct CachedJwks {
    fetched_at: DateTime<Utc>,
    keys: JwkSet,
}

/// Typed Prolific API boundary with bounded HTTP and daily JWK caching.
#[derive(Clone)]
pub struct ProlificClient {
    http: Client,
    api_base: String,
    token: String,
    jwks: Arc<RwLock<Option<CachedJwks>>>,
}

impl ProlificClient {
    /// Creates a client against the installation's configured Prolific service origin.
    pub fn with_base_url(token: impl Into<String>, api_base: impl Into<String>) -> Result<Self> {
        let token = token.into();
        if token.trim().is_empty() {
            bail!("Prolific API token is missing");
        }
        let http = Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self {
            http,
            api_base: api_base.into().trim_end_matches('/').to_string(),
            token,
            jwks: Arc::new(RwLock::new(None)),
        })
    }

    /// Retrieves the exact configured study without retaining unrelated response fields.
    pub async fn study(&self, study_id: &str) -> Result<Study> {
        self.authenticated_get(&format!("/api/v1/studies/{study_id}/"))
            .await
    }

    /// Retrieves one authoritative submission by the Prolific `SESSION_ID`.
    pub async fn submission(&self, session_id: &str) -> Result<Submission> {
        self.authenticated_get(&format!("/api/v1/submissions/{session_id}/"))
            .await
    }

    /// Retrieves one workspace after its identifier has been established by study setup.
    pub async fn workspace(&self, workspace_id: &str) -> Result<Workspace> {
        self.authenticated_get(&format!("/api/v1/workspaces/{workspace_id}/"))
            .await
    }

    /// Retrieves the project that owns a configured study.
    pub async fn project(&self, project_id: &str) -> Result<Project> {
        self.authenticated_get(&format!("/api/v1/projects/{project_id}/"))
            .await
    }

    /// Verifies a Secure external URL token and returns its fixed Prolific identifiers.
    pub async fn verify_signed_launch(
        &self,
        token: &str,
        expected: SignedLaunchExpectation<'_>,
    ) -> Result<VerifiedLaunch> {
        let header = decode_header(token).context("invalid Prolific launch token header")?;
        if header.alg != Algorithm::RS256 {
            bail!("Prolific launch token must use RS256");
        }
        let kid = header
            .kid
            .as_deref()
            .ok_or_else(|| anyhow!("Prolific launch token has no key identifier"))?;
        let mut keys = self.cached_jwks(false).await?;
        let mut jwk = keys.find(kid).cloned();
        if jwk.is_none() {
            keys = self.cached_jwks(true).await?;
            jwk = keys.find(kid).cloned();
        }
        let jwk = jwk.ok_or_else(|| anyhow!("Prolific launch signing key is unknown"))?;
        let decoding_key = DecodingKey::from_jwk(&jwk)?;
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&[PROLIFIC_ISSUER]);
        validation.set_audience(&[expected.audience]);
        validation.validate_exp = true;
        validation.required_spec_claims = ["exp", "iss", "aud", "sub"]
            .into_iter()
            .map(str::to_string)
            .collect();
        let claims = decode::<SignedLaunchClaims>(token, &decoding_key, &validation)
            .context("invalid or expired Prolific launch token")?
            .claims;
        if claims.sub != claims.prolific.session_id {
            bail!("Prolific launch subject does not match SESSION_ID");
        }
        if claims.prolific.study_id != expected.study_id {
            bail!("Prolific launch study does not match this experiment");
        }
        if claims.prolific.workspace_id != expected.workspace_id {
            bail!("Prolific launch workspace does not match this installation");
        }
        Ok(VerifiedLaunch {
            participant_id: claims.prolific.participant_id,
            study_id: claims.prolific.study_id,
            session_id: claims.prolific.session_id,
            workspace_id: claims.prolific.workspace_id,
            organisation_id: claims.prolific.organisation_id,
        })
    }

    /// Verifies an unsigned launch against the authoritative submission endpoint.
    pub async fn verify_unsigned_launch(
        &self,
        participant_id: &str,
        study_id: &str,
        session_id: &str,
    ) -> Result<Submission> {
        let submission = self.submission(session_id).await?;
        if submission.id != session_id
            || submission.study_id != study_id
            || submission.participant.as_deref() != Some(participant_id)
        {
            bail!("Prolific submission does not match the launch identifiers");
        }
        Ok(submission)
    }

    /// Returns activation issues for the five fixed Parlando completion paths.
    pub fn completion_path_issues(study: &Study, configured: &HashMap<&str, &str>) -> Vec<String> {
        let expected_actions = [
            ("completed", "AUTOMATICALLY_APPROVE"),
            ("partner_left", "AUTOMATICALLY_APPROVE"),
            ("partner_unavailable", "REQUEST_RETURN"),
            ("timed_out", "REQUEST_RETURN"),
            ("technical_failure", "AUTOMATICALLY_APPROVE"),
        ];
        let mut issues = Vec::new();
        for (field, expected_action) in expected_actions {
            let Some(configured_code) = configured.get(field).copied() else {
                issues.push(format!("Prolific completion field {field} is missing."));
                continue;
            };
            let Some(path) = study
                .completion_codes
                .iter()
                .find(|path| path.code.as_deref() == Some(configured_code))
            else {
                issues.push(format!(
                    "Prolific completion code for {field} does not exist in the linked study."
                ));
                continue;
            };
            let actions = path
                .actions
                .iter()
                .map(|action| action.action.as_str())
                .collect::<Vec<_>>();
            if actions != [expected_action] {
                issues.push(format!(
                    "Prolific completion code for {field} must have exactly the {expected_action} action."
                ));
            }
            if field == "partner_unavailable" && path.code_type == "FIXED_SCREENOUT" {
                issues.push(
                    "The Unmatched completion path must not use Prolific Screened out.".to_string(),
                );
            }
        }
        issues
    }

    /// Performs one authenticated GET and decodes only the requested allowlisted type.
    async fn authenticated_get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let response = self
            .http
            .get(format!("{}{path}", self.api_base))
            .header("Authorization", format!("Token {}", self.token))
            .send()
            .await
            .context("Prolific API request failed")?;
        let status = response.status();
        if !status.is_success() {
            let category = match status {
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => "authorization failed",
                StatusCode::NOT_FOUND => "resource was not found",
                StatusCode::TOO_MANY_REQUESTS => "rate limit was reached",
                _ if status.is_server_error() => "service is temporarily unavailable",
                _ => "request was rejected",
            };
            bail!("Prolific API {category} ({status})");
        }
        response
            .json::<T>()
            .await
            .context("Prolific API returned an invalid response")
    }

    /// Returns cached signing keys, refreshing daily or after an unknown key identifier.
    async fn cached_jwks(&self, force_refresh: bool) -> Result<JwkSet> {
        if !force_refresh {
            if let Some(cached) = self.jwks.read().await.as_ref() {
                if cached.fetched_at > Utc::now() - chrono::Duration::seconds(JWKS_TTL_SECONDS) {
                    return Ok(cached.keys.clone());
                }
            }
        }
        let response = self
            .http
            .get(format!("{}/.well-known/study/jwks.json", self.api_base))
            .send()
            .await
            .context("could not retrieve Prolific signing keys")?;
        if !response.status().is_success() {
            bail!(
                "could not retrieve Prolific signing keys ({})",
                response.status()
            );
        }
        let keys = response
            .json::<JwkSet>()
            .await
            .context("Prolific returned invalid signing keys")?;
        *self.jwks.write().await = Some(CachedJwks {
            fetched_at: Utc::now(),
            keys: keys.clone(),
        });
        Ok(keys)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Confirms preflight accepts only the exact five agreed provider actions.
    #[test]
    fn completion_path_preflight_is_exact() {
        let codes = [
            ("completed", "DONE", "AUTOMATICALLY_APPROVE"),
            ("partner_left", "PEER", "AUTOMATICALLY_APPROVE"),
            ("partner_unavailable", "NONE", "REQUEST_RETURN"),
            ("timed_out", "LATE", "REQUEST_RETURN"),
            ("technical_failure", "FAULT", "AUTOMATICALLY_APPROVE"),
        ];
        let study = Study {
            id: "study".to_string(),
            external_study_url: "https://example.test".to_string(),
            prolific_id_option: "url_parameters".to_string(),
            completion_codes: codes
                .iter()
                .map(|(_, code, action)| CompletionCode {
                    code: Some((*code).to_string()),
                    code_type: "OTHER".to_string(),
                    actions: vec![CompletionAction {
                        action: (*action).to_string(),
                    }],
                })
                .collect(),
            estimated_completion_time: 10,
            maximum_allowed_time: Some(30.0),
            is_external_study_url_secure: false,
            device_compatibility: vec![],
            peripheral_requirements: vec![],
            project: None,
            status: Some("UNPUBLISHED".to_string()),
        };
        let configured = codes
            .iter()
            .map(|(field, code, _)| (*field, *code))
            .collect::<HashMap<_, _>>();
        assert!(ProlificClient::completion_path_issues(&study, &configured).is_empty());
    }

    /// Confirms Unmatched cannot be implemented through Prolific's screen-out mechanism.
    #[test]
    fn unmatched_screenout_is_rejected() {
        let study = Study {
            id: "study".to_string(),
            external_study_url: "https://example.test".to_string(),
            prolific_id_option: "url_parameters".to_string(),
            completion_codes: vec![CompletionCode {
                code: Some("NONE".to_string()),
                code_type: "FIXED_SCREENOUT".to_string(),
                actions: vec![CompletionAction {
                    action: "REQUEST_RETURN".to_string(),
                }],
            }],
            estimated_completion_time: 10,
            maximum_allowed_time: Some(30.0),
            is_external_study_url_secure: false,
            device_compatibility: vec![],
            peripheral_requirements: vec![],
            project: None,
            status: None,
        };
        let configured = HashMap::from([("partner_unavailable", "NONE")]);
        assert!(ProlificClient::completion_path_issues(&study, &configured)
            .iter()
            .any(|issue| issue.contains("must not use Prolific Screened out")));
    }
}
