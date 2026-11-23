//! Durable participant identities, recruitment submissions and consent declarations.
//!
//! Intake uses these operations to persist a participant identity and consent declarations
//! and to resolve returning Prolific submissions. Session membership links that identity to a
//! particular room and role. Recruitment correlation and research identifiers have distinct
//! storage fields and uses.

use super::{now_iso, sqlite_unique_constraint_for, SqliteExperimentStore};
use crate::identity::new_id;
use crate::readable_id::participant_id as readable_participant_id;
use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Input for upserting a durable participant identity.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ParticipantRecord {
    /// Experiment within which exports reuse this participant identifier.
    pub experiment_id: String,
    pub participant_kind: String,
    pub identity_provider: String,
    pub external_id: Option<String>,
    pub metadata: Value,
}

/// Verified Prolific launch facts used to create or resume one durable admission.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ProlificSubmissionRecord {
    pub experiment_id: String,
    pub prolific_participant_id: String,
    pub prolific_study_id: String,
    pub prolific_session_id: String,
    /// `signed_url` or `submission_api`, recorded without retaining a launch token.
    pub verification_method: String,
}

/// Stable provider-neutral admission returned for one verified Prolific submission.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ProlificAdmission {
    pub participant_id: i64,
    pub research_id: String,
    pub participant_session_id: String,
}
/// Input for one item-level consent declaration.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ConsentDeclarationRecord {
    pub experiment_id: String,
    pub session_id: Option<i64>,
    pub participant_id: i64,
    /// Immutable data-use classification inherited from participant intake.
    pub purpose: String,
    pub consent_item_id: String,
    pub accepted: bool,
    pub consent_text_hash: Option<String>,
    pub metadata: Value,
}

/// Builds a non-human identifier from its single current durable representation.
fn nonhuman_participant_identifier(participant: &ParticipantRecord) -> Result<String> {
    if participant.participant_kind == "agent" {
        let metadata = participant
            .metadata
            .as_object()
            .ok_or_else(|| anyhow!("agent metadata must be an object"))?;
        let required = |key: &str| {
            metadata
                .get(key)
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| anyhow!("agent metadata requires {key:?}"))
        };
        return Ok(format!(
            "agent:{}:{}@{}",
            identifier_component(required("factory")?),
            identifier_component(required("agent_name")?),
            identifier_component(required("agent_version")?),
        ));
    }
    let external_id = participant
        .external_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("non-human participants require external_id"))?;
    Ok(format!(
        "{}:{}@{}",
        identifier_component(&participant.participant_kind),
        identifier_component(&participant.identity_provider),
        identifier_component(external_id),
    ))
}

/// Restricts one externally supplied identifier component to a compact display-safe alphabet.
fn identifier_component(value: &str) -> String {
    let mut component = String::new();
    let mut previous_was_separator = false;
    for character in value.trim().chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
            component.push(character);
            previous_was_separator = false;
        } else if !previous_was_separator && !component.is_empty() {
            component.push('-');
            previous_was_separator = true;
        }
    }
    while component.ends_with('-') {
        component.pop();
    }
    if component.is_empty() {
        "unknown".to_string()
    } else {
        component
    }
}

/// Produces a unique candidate, using random names only for human participants.
fn participant_identifier_candidate(
    participant: &ParticipantRecord,
    attempt: usize,
) -> Result<String> {
    if participant.participant_kind == "human" {
        Ok(readable_participant_id())
    } else {
        let base = nonhuman_participant_identifier(participant)?;
        if attempt == 1 {
            Ok(base)
        } else {
            Ok(format!("{base}~{attempt}"))
        }
    }
}

impl SqliteExperimentStore {
    /// Resolves a public identifier only when the participant belongs to this experiment.
    pub(crate) async fn participant_id_for_research_id(
        &self,
        experiment_id: &str,
        research_id: &str,
    ) -> Result<Option<i64>> {
        Ok(sqlx::query_scalar(
            "select p.participant_id
             from participants p
             where p.research_id = ? and exists (
                 select 1 from session_participants sp
                 where sp.experiment_id = ? and sp.participant_id = p.participant_id
             )",
        )
        .bind(research_id)
        .bind(experiment_id)
        .fetch_optional(&self.pool)
        .await?)
    }

    /// Records the latest provider status of a recruited submission.
    pub(crate) async fn reconcile_prolific_submission(
        &self,
        experiment_id: &str,
        prolific_session_id: &str,
        status: &str,
        entered_code: Option<String>,
        return_requested_at: Option<String>,
    ) -> Result<()> {
        sqlx::query(
            "update prolific_submissions set provider_status = ?, entered_completion_code = ?, return_requested_at = ?, reconciled_at = ? where experiment_id = ? and prolific_session_id = ?",
        )
        .bind(status)
        .bind(entered_code)
        .bind(return_requested_at)
        .bind(now_iso())
        .bind(experiment_id)
        .bind(prolific_session_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
    /// Resolves a durable participant identifier or allocates a new identity.
    pub(crate) async fn upsert_participant(&self, participant: ParticipantRecord) -> Result<i64> {
        if let Some(external_id) = participant.external_id.as_deref() {
            if let Some(participant_id) = sqlx::query_scalar::<_, i64>(
                "select participant_id from participants where experiment_id = ? and identity_provider = ? and external_id = ?",
            )
            .bind(&participant.experiment_id)
            .bind(&participant.identity_provider)
            .bind(external_id)
            .fetch_optional(&self.pool)
            .await?
            {
                return Ok(participant_id);
            }
        }
        let mut identifier_attempt = 1;
        loop {
            let identifier = participant_identifier_candidate(&participant, identifier_attempt)?;
            let result = sqlx::query(
                r#"
                insert into participants
                (research_id, experiment_id, participant_kind, identity_provider, external_id, metadata_json, created_at)
                values (?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(identifier)
            .bind(&participant.experiment_id)
            .bind(&participant.participant_kind)
            .bind(&participant.identity_provider)
            .bind(&participant.external_id)
            .bind(serde_json::to_string(&participant.metadata)?)
            .bind(now_iso())
            .execute(&self.pool)
            .await;
            match result {
                Ok(result) => return Ok(result.last_insert_rowid()),
                Err(error) if sqlite_unique_constraint_for(&error, "participants.research_id") => {
                    identifier_attempt += 1;
                    continue;
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    /// Reuses a verified submission, allocating durable identity only when intake has capacity.
    pub(crate) async fn admit_prolific_submission(
        &self,
        submission: ProlificSubmissionRecord,
        allow_new: bool,
    ) -> Result<Option<ProlificAdmission>> {
        let mut tx = self.pool.begin().await?;
        let existing = sqlx::query_as::<_, (i64, String, String, String, String)>(
            r#"
            select ps.participant_id, p.research_id, ps.participant_session_id,
                   ps.prolific_participant_id, ps.prolific_study_id
            from prolific_submissions ps
            join participants p on p.participant_id = ps.participant_id
            where ps.experiment_id = ? and ps.prolific_session_id = ?
            "#,
        )
        .bind(&submission.experiment_id)
        .bind(&submission.prolific_session_id)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some((participant_id, research_id, participant_session_id, participant, study)) =
            existing
        {
            if participant != submission.prolific_participant_id
                || study != submission.prolific_study_id
            {
                bail!("Prolific submission identifiers conflict with an existing admission");
            }
            tx.commit().await?;
            return Ok(Some(ProlificAdmission {
                participant_id,
                research_id,
                participant_session_id,
            }));
        }

        if !allow_new {
            return Ok(None);
        }
        let participant_record = ParticipantRecord {
            experiment_id: submission.experiment_id.clone(),
            participant_kind: "human".to_string(),
            identity_provider: "prolific".to_string(),
            external_id: None,
            metadata: Value::Null,
        };
        let (participant_id, research_id) = {
            let mut identifier_attempt = 1;
            loop {
                let research_id =
                    participant_identifier_candidate(&participant_record, identifier_attempt)?;
                let result = sqlx::query(
                    r#"
                    insert into participants
                    (research_id, experiment_id, participant_kind, identity_provider, external_id, metadata_json, created_at)
                    values (?, ?, 'human', 'prolific', null, ?, ?)
                    "#,
                )
                .bind(&research_id)
                .bind(&submission.experiment_id)
                .bind(serde_json::to_string(&Value::Null)?)
                .bind(now_iso())
                .execute(&mut *tx)
                .await;
                match result {
                    Ok(result) => break (result.last_insert_rowid(), research_id),
                    Err(error)
                        if sqlite_unique_constraint_for(&error, "participants.research_id") =>
                    {
                        identifier_attempt += 1;
                    }
                    Err(error) => return Err(error.into()),
                }
            }
        };
        let participant_session_id = new_id("ps");
        let verified_at = now_iso();
        sqlx::query(
            r#"
            insert into prolific_submissions
                (experiment_id, participant_id, prolific_participant_id, prolific_study_id,
                 prolific_session_id, participant_session_id, received_at,
                 verification_method, verified_at)
            values (?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&submission.experiment_id)
        .bind(participant_id)
        .bind(&submission.prolific_participant_id)
        .bind(&submission.prolific_study_id)
        .bind(&submission.prolific_session_id)
        .bind(&participant_session_id)
        .bind(&verified_at)
        .bind(&submission.verification_method)
        .bind(&verified_at)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(Some(ProlificAdmission {
            participant_id,
            research_id,
            participant_session_id,
        }))
    }

    /// Looks up the pseudonymous research identifier for a participant.
    pub(crate) async fn participant_research_id(
        &self,
        participant_id: i64,
    ) -> Result<Option<String>> {
        Ok(sqlx::query_scalar::<_, String>(
            "select research_id from participants where participant_id = ?",
        )
        .bind(participant_id)
        .fetch_optional(&self.pool)
        .await?)
    }
    /// Writes one participant decision about a consent item.
    pub(crate) async fn record_consent_declaration(
        &self,
        declaration: ConsentDeclarationRecord,
    ) -> Result<()> {
        sqlx::query(
            r#"
            insert into consent_declarations
            (experiment_id, session_id, participant_id, consent_item_id, accepted, purpose, declared_at, consent_text_hash, metadata_json)
            values (?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(declaration.experiment_id)
        .bind(declaration.session_id)
        .bind(declaration.participant_id)
        .bind(declaration.consent_item_id)
        .bind(if declaration.accepted { 1 } else { 0 })
        .bind(declaration.purpose)
        .bind(now_iso())
        .bind(declaration.consent_text_hash)
        .bind(serde_json::to_string(&declaration.metadata)?)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Confirms agent identifiers expose the configured type, implementation name, and version.
    #[test]
    fn agent_identifier_uses_durable_type_and_version_metadata() {
        let participant = ParticipantRecord {
            experiment_id: "experiment-a".to_string(),
            participant_kind: "agent".to_string(),
            identity_provider: "remote_grpc".to_string(),
            external_id: Some("Python Agent@v1.2 beta".to_string()),
            metadata: json!({
                "factory": "remote_grpc",
                "agent_name": "Python Agent",
                "agent_version": "v1.2 beta",
            }),
        };

        assert_eq!(
            participant_identifier_candidate(&participant, 1).unwrap(),
            "agent:remote_grpc:Python-Agent@v1.2-beta"
        );
        assert_eq!(
            participant_identifier_candidate(&participant, 2).unwrap(),
            "agent:remote_grpc:Python-Agent@v1.2-beta~2"
        );

        let unversioned = ParticipantRecord {
            external_id: Some("Python Agent".to_string()),
            metadata: json!({
                "agent_type": "remote_grpc",
                "agent_name": "Python Agent",
            }),
            ..participant
        };
        assert!(participant_identifier_candidate(&unversioned, 1).is_err());
    }
}
