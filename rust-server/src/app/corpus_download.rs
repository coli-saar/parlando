//! Writes one validated corpus to a private spool before sending the HTTP response.
//! The SQLite read snapshot is released even if a client stops consuming the body.

use super::{
    corpus::{corpus_event, corpus_participant, corpus_session_metadata},
    redact_secret_fields, AppError, StatusCode,
};
use crate::storage::{CorpusInventory, CorpusRead};
use axum::body::{Body, Bytes};
use futures_util::{stream, TryStreamExt};
use serde_json::{json, Value};
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufWriter},
};

/// The supported serializations of the same corpus structure.
#[derive(Clone, Copy)]
pub(super) enum CorpusFormat {
    Json,
    Yaml,
    Csv,
}

impl CorpusFormat {
    /// Resolves the public format parameter before any database read.
    pub(super) fn parse(format: Option<&str>) -> Result<Self, AppError> {
        match format.unwrap_or("json") {
            "json" => Ok(Self::Json),
            "yaml" | "yml" => Ok(Self::Yaml),
            "csv" => Ok(Self::Csv),
            other => Err(AppError::bad_request(format!(
                "Unsupported export format {other:?}."
            ))),
        }
    }

    /// Returns the filename suffix and HTTP media type for this format.
    pub(super) fn media_type(self) -> (&'static str, &'static str) {
        match self {
            Self::Json => ("json", "application/json; charset=utf-8"),
            Self::Yaml => ("yaml", "application/yaml; charset=utf-8"),
            Self::Csv => ("csv", "text/csv; charset=utf-8"),
        }
    }
}

/// Builds and validates a fully framed artifact without retaining a whole session in memory.
pub(super) async fn download_corpus(
    mut read: CorpusRead,
    experiment_id: &str,
    privacy_contract_version: &str,
    format: CorpusFormat,
) -> Result<(u64, Body), AppError> {
    let (status, version, config) = read
        .experiment()
        .await?
        .ok_or_else(|| AppError::not_found("Configured experiment not found."))?;
    if status == "archived" {
        return Err(AppError::new(
            StatusCode::CONFLICT,
            "Archived experiments must be restored before export",
        ));
    }
    let inventory = read.inventory().await?;
    let has_participants = inventory.participants != 0;
    let has_sessions = inventory.sessions != 0;
    let experiment = json!({
        "experiment_id": experiment_id,
        "game": {"version": version},
        "configuration": config.get("game").cloned().unwrap_or_else(|| json!({})),
    });
    let file = File::from_std(tempfile::tempfile().map_err(anyhow::Error::from)?);
    let mut writer = CorpusWriter {
        file: BufWriter::with_capacity(64 * 1024, file),
        format,
    };
    writer
        .begin(
            manifest(privacy_contract_version, inventory),
            experiment,
            has_participants,
        )
        .await?;
    {
        let mut rows = read.participants();
        let mut first = true;
        while let Some((id, kind, metadata)) = rows.try_next().await? {
            let participant =
                corpus_participant(id.as_deref(), &kind, &metadata.unwrap_or(Value::Null))
                    .map_err(projection_error)?;
            writer.item(participant, first, "    ").await?;
            first = false;
        }
    }
    writer.begin_sessions(has_sessions).await?;
    let mut after = None;
    while let Some(session) = read.next_session(after).await? {
        let first_session = after.is_none();
        after = Some(session.session_id);
        if session.dialogue_id.is_empty() {
            return Err(projection_error(anyhow::anyhow!(
                "Corpus export integrity error: a session lacks its dashboard ID"
            )));
        }
        let assignments = read.assignments(session.session_id).await?;
        if assignments.len() != 2 {
            return Err(projection_error(anyhow::anyhow!(
                "Corpus export integrity error: session {} has {} participant assignments; expected exactly two",
                session.dialogue_id, assignments.len()
            )));
        }
        let participants = assignments
            .into_iter()
            .map(|(id, role)| {
                let id = id.ok_or_else(|| {
                    projection_error(anyhow::anyhow!(
                "Corpus export integrity error: a session assignment has no exported participant"
            ))
                })?;
                Ok(json!({"participant_id": id, "role": role}))
            })
            .collect::<Result<Vec<_>, AppError>>()?;
        let metadata = corpus_session_metadata(&session).map_err(projection_error)?;
        writer
            .begin_session(
                json!({
                    "session_id": session.dialogue_id,
                    "metadata": metadata,
                    "participants": participants,
                    "completion": session.completion,
                }),
                first_session,
            )
            .await?;
        read.index_timings(session.session_id).await?;
        let mut events = read.events(session.session_id);
        let mut first_event = true;
        while let Some((row, participant, timing)) = events.try_next().await? {
            let event = corpus_event(
                &session.dialogue_id,
                &row,
                participant.as_deref(),
                timing.as_ref(),
            )
            .map_err(projection_error)?;
            if first_event {
                writer.begin_events().await?;
            }
            writer.item(event, first_event, "        ").await?;
            first_event = false;
        }
        writer.end_session(first_event).await?;
    }
    read.finish().await?;
    let file = writer.finish().await?;
    let bytes = file.metadata().await.map_err(anyhow::Error::from)?.len();
    Ok((bytes, body_from_spool(file)))
}

/// Preserves the distinction between an inconsistent corpus and an infrastructure failure.
fn projection_error(error: anyhow::Error) -> AppError {
    AppError::new(StatusCode::CONFLICT, error.to_string())
}

/// Builds the public manifest from counts taken inside the artifact's snapshot.
fn manifest(privacy_contract_version: &str, counts: CorpusInventory) -> Value {
    json!({
        "export_schema_version": "parlando.corpus.v1",
        "export_variant": "corpus",
        "release_status": "corpus_candidate",
        "content_review_required": true,
        "privacy_contract_version": privacy_contract_version,
        "data_inventory": {
            "participants": counts.participants,
            "sessions": counts.sessions,
            "events": counts.actions + counts.messages + counts.logs,
            "actions": counts.actions,
            "messages": counts.messages,
            "logs": counts.logs,
        },
    })
}

/// Owns the format framing. Record payloads are redacted at the serialization boundary.
struct CorpusWriter {
    format: CorpusFormat,
    file: BufWriter<File>,
}

impl CorpusWriter {
    /// Writes the manifest and the experiment header, opening its participant sequence.
    async fn begin(
        &mut self,
        manifest: Value,
        experiment: Value,
        has_participants: bool,
    ) -> anyhow::Result<()> {
        match self.format {
            CorpusFormat::Yaml => {
                self.yaml(manifest, "").await?;
                self.write("experiment:\n").await?;
                self.yaml(experiment, "  ").await?;
                self.write(if has_participants {
                    "  participants:\n"
                } else {
                    "  participants: []\n"
                })
                .await
            }
            CorpusFormat::Json => {
                self.json_object_prefix(manifest).await?;
                self.write(",\"experiment\":").await?;
                self.json_object_prefix(experiment).await?;
                self.write(",\"participants\":[").await
            }
            CorpusFormat::Csv => {
                self.write("table,row_json\nmanifest,\"").await?;
                self.json(manifest).await?;
                self.write("\"\nexperiment,\"").await?;
                self.json_object_prefix(experiment).await?;
                self.json_fragment(",\"participants\":[").await
            }
        }
    }

    /// Closes the registry and opens the sessions sequence or ends the CSV experiment row.
    async fn begin_sessions(&mut self, has_sessions: bool) -> anyhow::Result<()> {
        match self.format {
            CorpusFormat::Csv => self.write("]}\"\n").await,
            CorpusFormat::Json => self.write("],\"sessions\":[").await,
            CorpusFormat::Yaml => {
                self.write(if has_sessions {
                    "  sessions:\n"
                } else {
                    "  sessions: []\n"
                })
                .await
            }
        }
    }

    /// Starts a session value, leaving room for its chronological events.
    async fn begin_session(&mut self, session: Value, first: bool) -> anyhow::Result<()> {
        match self.format {
            CorpusFormat::Yaml => self.yaml(Value::Array(vec![session]), "    ").await,
            CorpusFormat::Json | CorpusFormat::Csv => {
                if matches!(self.format, CorpusFormat::Csv) {
                    self.write("session,\"").await?;
                } else if !first {
                    self.write(",").await?;
                }
                self.json_object_prefix(session).await
            }
        }
    }

    /// Opens a nonempty events sequence.
    async fn begin_events(&mut self) -> anyhow::Result<()> {
        match self.format {
            CorpusFormat::Yaml => self.write("      events:\n").await,
            _ => self.json_fragment(",\"events\":[").await,
        }
    }

    /// Adds a participant or event to the enclosing sequence.
    async fn item(&mut self, value: Value, first: bool, yaml_indent: &str) -> anyhow::Result<()> {
        match self.format {
            CorpusFormat::Yaml => self.yaml(Value::Array(vec![value]), yaml_indent).await,
            _ => {
                if !first {
                    self.write(",").await?;
                }
                self.json(value).await
            }
        }
    }

    /// Closes the event sequence and its session, accounting for empty histories.
    async fn end_session(&mut self, no_events: bool) -> anyhow::Result<()> {
        match self.format {
            CorpusFormat::Yaml => {
                if no_events {
                    self.write("      events: []\n").await?;
                }
            }
            _ => {
                if no_events {
                    self.begin_events().await?;
                }
                self.write("]}").await?;
                if matches!(self.format, CorpusFormat::Csv) {
                    self.write("\"\n").await?;
                }
            }
        }
        Ok(())
    }

    /// Finalizes the enclosing document and rewinds its private file for response streaming.
    async fn finish(mut self) -> anyhow::Result<File> {
        if matches!(self.format, CorpusFormat::Json) {
            self.write("]}}").await?;
        }
        self.file.flush().await?;
        let mut file = self.file.into_inner();
        file.rewind().await?;
        Ok(file)
    }

    /// Writes an object without its closing brace so arrays can be appended incrementally.
    async fn json_object_prefix(&mut self, mut value: Value) -> anyhow::Result<()> {
        redact_secret_fields(&mut value);
        let mut encoded = serde_json::to_string(&value)?;
        encoded.pop();
        self.json_fragment(&encoded).await
    }

    /// Serializes a complete redacted JSON value.
    async fn json(&mut self, mut value: Value) -> anyhow::Result<()> {
        redact_secret_fields(&mut value);
        self.json_fragment(&serde_json::to_string(&value)?).await
    }

    /// Doubles quotes for fragments inside a CSV row's JSON column.
    async fn json_fragment(&mut self, fragment: &str) -> anyhow::Result<()> {
        if matches!(self.format, CorpusFormat::Csv) {
            self.write(&fragment.replace('"', "\"\"")).await
        } else {
            self.write(fragment).await
        }
    }

    /// Serializes a redacted YAML fragment and indents it into the enclosing document.
    async fn yaml(&mut self, mut value: Value, indent: &str) -> anyhow::Result<()> {
        redact_secret_fields(&mut value);
        let fragment = serde_yaml::to_string(&value)?;
        for line in fragment.split_inclusive('\n') {
            self.write(indent).await?;
            self.write(line).await?;
        }
        Ok(())
    }

    /// Buffers framing and serialized fragments before writing them to disk.
    async fn write(&mut self, text: &str) -> anyhow::Result<()> {
        self.file.write_all(text.as_bytes()).await?;
        Ok(())
    }
}

/// Reads one bounded chunk per poll; dropping the response drops the private file.
fn body_from_spool(file: File) -> Body {
    Body::from_stream(stream::try_unfold(file, |mut file| async move {
        let mut buffer = vec![0; 64 * 1024];
        let count = file.read(&mut buffer).await?;
        buffer.truncate(count);
        Ok::<_, std::io::Error>((count != 0).then_some((Bytes::from(buffer), file)))
    }))
}
