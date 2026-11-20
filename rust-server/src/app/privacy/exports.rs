//! Describes the research corpus in the privacy report. The report builder supplies the
//! enabled features. The schema fingerprint comes from the schema embedded by the corpus
//! projector, which owns the actual export format.

use super::{PrivacyExportFieldStatus, PrivacyExportStatus};
use crate::app::corpus::CORPUS_EXPORT_SCHEMA_V1;
use sha2::{Digest, Sha256};

/// Converts a static corpus-schema field list into the serialized privacy descriptor.
fn privacy_field_names(fields: &[&str]) -> Vec<String> {
    fields.iter().map(|field| (*field).to_string()).collect()
}

/// Describes the corpus schema and the content review required for this configuration.
pub(super) fn export_status(
    agent_session: bool,
    transcription: usize,
    tts: bool,
    has_external_services: bool,
) -> PrivacyExportStatus {
    PrivacyExportStatus {
            available: true,
            variant: "corpus".to_string(),
            schema_id: "parlando.corpus.v1".to_string(),
            schema_sha256: format!("{:x}", Sha256::digest(CORPUS_EXPORT_SCHEMA_V1.as_bytes())),
            scope: "Research sessions in the selected experiment that entered gameplay.".to_string(),
            release_status: "corpus_candidate".to_string(),
            content_review_required: true,
            formats: vec!["JSON".to_string(), "YAML".to_string(), "CSV".to_string()],
            identifiers: "Human participants are represented by Parlando's randomly generated three-word pseudonyms, and sessions by separate randomly generated three-word pseudonyms. These identifiers are not derived from names, contact details, IP addresses, or external participant identifiers. Software agents use an identifier derived from their agent implementation rather than a human identity. Internal numeric keys, session identifiers, and temporary connection identifiers are not written to the corpus. The corpus is pseudonymized, not anonymous: participant-authored text can still contain identifying information and must be reviewed before sharing.".to_string(),
            structure: "The corpus contains one experiment record with game information and configuration, a list of participants, and a list of sessions. Each session contains its own context, both participant-role assignments, the game-supplied completion record, and ordered accepted-action, participant-message, and game/agent log events.".to_string(),
            timing: "Session events are stored and exported as game_time_ms, measured in milliseconds from game start. Transcribed speech start and end positions use the same game clock. Waiting time and session duration are derived from lifecycle timestamps. Running sessions have null duration. Logs timestamped before game start are retained in the dashboard history and excluded from the corpus. Game-defined configuration, actions, transition information, state, and completion are included unchanged.".to_string(),
            selection_rule: "Parlando builds a new corpus document and writes only the categories listed below. It does not copy a database-shaped document and then remove columns. A database field that has no listed destination is never written to the corpus.".to_string(),
            included_fields: vec![
                PrivacyExportFieldStatus {
                    section: "Manifest".to_string(),
                    description: "The export format and Privacy Contract versions, the reminder to review participant utterances for explicit identifying information, and summary totals for participants, sessions, and exported events."
                        .to_string(),
                    fields: privacy_field_names(&[
                        "export_schema_version",
                        "export_variant",
                        "release_status",
                        "content_review_required",
                        "privacy_contract_version",
                        "data_inventory.{participants,sessions,events,actions,messages,logs}",
                    ]),
                },
                PrivacyExportFieldStatus {
                    section: "Experiment".to_string(),
                    description: "The experiment label shown in the dashboard, game version, complete game-specific configuration, participant catalogue, and sessions."
                        .to_string(),
                    fields: privacy_field_names(&[
                        "experiment_id",
                        "game.version",
                        "configuration (complete game-owned value)",
                        "participants",
                        "sessions",
                    ]),
                },
                PrivacyExportFieldStatus {
                    section: "Participant".to_string(),
                    description: if agent_session {
                        "The human's randomly generated three-word pseudonym and the software agent's implementation-derived identifier. The agent entry also identifies its implementation and configuration."
                    } else {
                        "The two humans' randomly generated three-word pseudonyms."
                    }
                    .to_string(),
                    fields: if agent_session {
                        privacy_field_names(&[
                            "participant_id (dashboard ID)",
                            "kind",
                            "agent_identity.{factory_id,agent_name,agent_version,configuration_fingerprint}",
                        ])
                    } else {
                        privacy_field_names(&["participant_id (dashboard ID)", "kind=human"])
                    },
                },
                PrivacyExportFieldStatus {
                    section: "Session".to_string(),
                    description: "The randomly generated three-word session pseudonym, game and configuration versions, how the session was created, final status, relative waiting time and duration, both participant-role assignments, game-supplied completion record, and exported events."
                        .to_string(),
                    fields: privacy_field_names(&[
                        "session_id (dashboard ID)",
                        "metadata.{game_version,config_revision,mode,status,time_to_start_ms,duration_ms}",
                        "participants[].{participant_id,role}",
                        "completion (complete game-owned value)",
                        "events",
                    ]),
                },
                PrivacyExportFieldStatus {
                    section: "Action event".to_string(),
                    description: "Order and game_time_ms in milliseconds from game start, participant and role, complete accepted action, game-supplied transition information, and complete resulting game state."
                        .to_string(),
                    fields: privacy_field_names(&[
                        "index",
                        "game_time_ms",
                        "kind=action",
                        "participant_id",
                        "role",
                        "action (complete game-owned value)",
                        "transition_metadata (complete game-owned value)",
                        "state (complete game-owned value)",
                    ]),
                },
                PrivacyExportFieldStatus {
                    section: "Message event".to_string(),
                    description: if transcription > 0 || tts {
                        "Order and game_time_ms in milliseconds from game start, participant and role, whether the message was typed, transcribed from speech, or generated by an agent, message text, optional speech start and end positions on the same game clock, and independently timed spoken words and punctuation. Recognition confidence is available only for transcribed speech."
                    } else {
                        "Order and game_time_ms in milliseconds from game start, participant and role, typed message text, and its typed origin."
                    }
                    .to_string(),
                    fields: privacy_field_names(&[
                        "index",
                        "game_time_ms",
                        "kind=message",
                        "participant_id",
                        "role",
                        "origin",
                        "text",
                        "utterance_timing?.{origin=game_clock,start_ms,end_ms}",
                    "tokens?.[{kind,text,start_ms,end_ms,confidence?}]",
                    ]),
                },
                PrivacyExportFieldStatus {
                    section: "Log event".to_string(),
                    description: "Order and game_time_ms in milliseconds from game start, runtime-owned game or agent source attribution, optional agent participant and role, and complete game- or agent-authored log text."
                        .to_string(),
                    fields: privacy_field_names(&[
                        "index",
                        "game_time_ms",
                        "kind=log",
                        "source",
                        "participant_id (agent logs only)",
                        "role (agent logs only)",
                        "text",
                    ]),
                },
            ],
            not_written: vec![
                "Testing sessions and participants that occur only in testing sessions."
                    .to_string(),
                "Consent declarations, participant-information references, and the configured consent text."
                    .to_string(),
                "Database-only participant creation metadata.".to_string(),
                "Internal numeric participant and session keys, session identifiers, temporary participant-session identifiers, connection status, and join or leave times."
                    .to_string(),
                "Parlando wall-clock timestamps. The corpus writes stored game-clock event and utterance coordinates plus derived waiting and session durations."
                    .to_string(),
                if agent_session || has_external_services {
                    "Operational lifecycle, connection, rejected-input, and any enabled software-agent or speech-service status events."
                } else {
                    "Operational lifecycle, connection, and rejected-input events."
                }
                .to_string(),
                "Game- and agent-defined log text.".to_string(),
                "Administrator accounts and sessions, server access settings, provider credentials, and other administrator metadata."
                    .to_string(),
            ],
            detail: if transcription > 0 {
                "Before sharing the corpus, review participant-authored typed messages and final voice transcripts and remove explicit identifying information. Parlando does not perform this text review automatically."
            } else {
                "Before sharing the corpus, review participant-authored typed messages and remove explicit identifying information. Parlando does not perform this text review automatically."
            }
            .to_string(),
        }
}
