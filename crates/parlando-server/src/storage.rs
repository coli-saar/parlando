use std::{collections::HashMap, path::Path, sync::Arc};

use anyhow::{bail, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{sqlite::SqlitePoolOptions, SqlitePool};
use tokio::sync::RwLock;

use crate::{game::Seat, identity::new_id, protocol::ConversationMessageResponse};

pub fn now_iso() -> String {
    Utc::now().to_rfc3339()
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TimelineEvent {
    pub id: String,
    pub room_id: Option<String>,
    pub participant_session_id: Option<String>,
    pub event_type: String,
    pub payload: Value,
    pub created_at: DateTime<Utc>,
}

impl TimelineEvent {
    pub fn new(
        event_type: impl Into<String>,
        room_id: Option<String>,
        participant_session_id: Option<String>,
        payload: Value,
    ) -> Self {
        Self {
            id: new_id("tl"),
            room_id,
            participant_session_id,
            event_type: event_type.into(),
            payload,
            created_at: Utc::now(),
        }
    }
}

#[async_trait]
pub trait EventStore: Send + Sync {
    async fn record(&self, event: TimelineEvent) -> Result<()>;
    async fn export(&self) -> Result<Vec<TimelineEvent>>;
}

pub type SharedEventStore = Arc<dyn EventStore>;

#[derive(Default)]
pub struct MemoryEventStore {
    events: RwLock<Vec<TimelineEvent>>,
}

#[async_trait]
impl EventStore for MemoryEventStore {
    async fn record(&self, event: TimelineEvent) -> Result<()> {
        self.events.write().await.push(event);
        Ok(())
    }

    async fn export(&self) -> Result<Vec<TimelineEvent>> {
        Ok(self.events.read().await.clone())
    }
}

pub struct SqliteEventStore {
    pool: SqlitePool,
}

impl SqliteEventStore {
    pub async fn connect(database_url: &str) -> Result<Self> {
        if database_url.is_empty() || database_url == "sqlite:///:memory:" {
            let pool = SqlitePoolOptions::new().max_connections(5).connect("sqlite::memory:").await?;
            let store = Self { pool };
            store.ensure_schema().await?;
            return Ok(store);
        }
        if !database_url.starts_with("sqlite:///") {
            bail!("unsupported database url scheme for {database_url:?}; only sqlite:/// is implemented");
        }
        let path = database_url.trim_start_matches("sqlite:///");
        if let Some(parent) = Path::new(path).parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(&format!("sqlite://{path}"))
            .await?;
        let store = Self { pool };
        store.ensure_schema().await?;
        Ok(store)
    }

    async fn ensure_schema(&self) -> Result<()> {
        sqlx::query(
            r#"
            create table if not exists timeline_events (
                id text primary key,
                room_id text,
                participant_session_id text,
                event_type text not null,
                payload_json text not null,
                created_at text not null
            )
            "#,
        )
        .execute(&self.pool)
        .await?;
        sqlx::query("create index if not exists idx_timeline_room_created on timeline_events(room_id, created_at)")
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

#[async_trait]
impl EventStore for SqliteEventStore {
    async fn record(&self, event: TimelineEvent) -> Result<()> {
        sqlx::query(
            r#"
            insert into timeline_events
            (id, room_id, participant_session_id, event_type, payload_json, created_at)
            values (?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(event.id)
        .bind(event.room_id)
        .bind(event.participant_session_id)
        .bind(event.event_type)
        .bind(serde_json::to_string(&event.payload)?)
        .bind(event.created_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn export(&self) -> Result<Vec<TimelineEvent>> {
        let rows = sqlx::query_as::<_, (String, Option<String>, Option<String>, String, String, String)>(
            "select id, room_id, participant_session_id, event_type, payload_json, created_at from timeline_events order by created_at, id",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|(id, room_id, participant_session_id, event_type, payload_json, created_at)| {
                Ok(TimelineEvent {
                    id,
                    room_id,
                    participant_session_id,
                    event_type,
                    payload: serde_json::from_str(&payload_json)?,
                    created_at: DateTime::parse_from_rfc3339(&created_at)?.with_timezone(&Utc),
                })
            })
            .collect()
    }
}

pub async fn event_store_from_url(database_url: &str) -> Result<SharedEventStore> {
    if database_url.is_empty() {
        Ok(Arc::new(MemoryEventStore::default()))
    } else {
        Ok(Arc::new(SqliteEventStore::connect(database_url).await?))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ParticipantSession {
    pub id: String,
    pub source: String,
    pub status: String,
    pub display_name: Option<String>,
    pub study_id: Option<String>,
    pub consent_decisions: HashMap<String, bool>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RoomParticipant {
    pub participant_session_id: String,
    pub role: Seat,
    pub connected: bool,
    pub consent_decisions: HashMap<String, bool>,
    pub joined_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GameRoom<S> {
    pub id: String,
    pub mode: String,
    pub state: S,
    pub status: String,
    pub study_id: Option<String>,
    pub participants: HashMap<String, RoomParticipant>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TranscriptSegment {
    pub id: String,
    pub room_id: String,
    pub participant_session_id: String,
    pub player: String,
    pub start_time_ms: i64,
    pub end_time_ms: i64,
    pub move_count: Option<i64>,
    pub text: String,
    pub metadata: Value,
    pub created_at: String,
}

#[derive(Clone, Debug)]
pub struct MemoryState<S> {
    pub participants: HashMap<String, ParticipantSession>,
    pub rooms: HashMap<String, GameRoom<S>>,
    pub matchmaking_queues: HashMap<String, Vec<String>>,
    pub transcripts: Vec<TranscriptSegment>,
    pub conversation_messages: Vec<ConversationMessageResponse>,
    pub voice_diagnostics: Vec<Value>,
    pub completions: Vec<Value>,
}

impl<S> Default for MemoryState<S> {
    fn default() -> Self {
        Self {
            participants: HashMap::new(),
            rooms: HashMap::new(),
            matchmaking_queues: HashMap::new(),
            transcripts: vec![],
            conversation_messages: vec![],
            voice_diagnostics: vec![],
            completions: vec![],
        }
    }
}

impl<S: Clone + Serialize> MemoryState<S> {
    pub fn create_participant(&mut self, source: String, display_name: Option<String>, study_id: Option<String>) -> ParticipantSession {
        let now = now_iso();
        let participant = ParticipantSession {
            id: new_id("ps"),
            source,
            status: "created".to_string(),
            display_name,
            study_id,
            consent_decisions: HashMap::new(),
            created_at: now.clone(),
            updated_at: now,
        };
        self.participants.insert(participant.id.clone(), participant.clone());
        participant
    }

    pub fn room_for_participant(&self, participant_session_id: &str) -> Option<&GameRoom<S>> {
        self.rooms.values().find(|room| room.participants.contains_key(participant_session_id))
    }

    pub fn conversation_for_room(&self, room_id: &str, limit: usize) -> Vec<ConversationMessageResponse> {
        let mut messages = self
            .conversation_messages
            .iter()
            .filter(|message| message.room_id == room_id)
            .cloned()
            .collect::<Vec<_>>();
        if messages.len() > limit {
            messages = messages.split_off(messages.len() - limit);
        }
        messages
    }

    pub fn export_snapshot(&self, timeline: Vec<TimelineEvent>) -> Value {
        json!({
            "participants": self.participants,
            "rooms": self.rooms,
            "transcripts": self.transcripts,
            "conversation_messages": self.conversation_messages,
            "voice_diagnostics": self.voice_diagnostics,
            "completions": self.completions,
            "timeline": timeline,
        })
    }
}
