//! Row types persisted by `nexus-store`. These mirror the schema created in
//! `migrations.rs` field-for-field; see `docs/CONTRACTS.md` for the frozen
//! public shape other crates build against.

use chrono::{DateTime, Utc};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Conversation {
    pub id: String, // uuid v4 string
    pub title: String,
    pub folder: Option<String>,
    pub is_favorite: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Message {
    pub id: String,
    pub conversation_id: String,
    pub role: String, // "system" | "user" | "assistant" | "tool"
    pub content: String,
    pub provider_id: Option<String>,
    pub model_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Automation {
    pub id: String,
    pub name: String,
    /// Serialized `nexus_automation::AutomationDefinition` (owned by the
    /// automation crate; stored here as opaque JSON text).
    pub definition_json: String,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AutomationRun {
    pub id: String,
    pub automation_id: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub status: String,   // "running" | "success" | "failed" | "cancelled"
    pub log_json: String, // serialized Vec<StepLogEntry>
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UsageEvent {
    pub id: String,
    pub occurred_at: DateTime<Utc>,
    pub provider_id: String,
    pub model_id: String,
    pub kind: String, // "chat" | "cli" | "automation"
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub latency_ms: u32,
    pub success: bool,
    pub error_message: Option<String>,
}
