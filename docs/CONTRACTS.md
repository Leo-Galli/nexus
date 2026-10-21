# Cross-crate contracts

This file freezes the public API of crates that other, concurrently
written crates depend on, so independent implementers do not need to wait
on each other. Treat every signature below as fixed; if you need to
deviate, change this file first and note why.

`nexus-core`'s actual trait/type definitions are the source of truth and
already exist on disk at `crates/nexus-core/src/{provider,cli_agent,model,
error,secrets,http,redact}.rs` — read them directly rather than relying on
a paraphrase.

## `nexus-store` public API (crate `nexus-store`)

```rust
// crates/nexus-store/src/lib.rs — public surface only, implement exactly this.

pub struct Store { /* private: single tokio::sync::Mutex<rusqlite::Connection> */ }

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    Sqlite(#[from] rusqlite::Error),
    Serde(#[from] serde_json::Error),
}
pub type StoreResult<T> = Result<T, StoreError>;

impl Store {
    /// Opens (creating if needed) the SQLite file at `db_path` and runs
    /// all pending migrations before returning. Uses
    /// `tokio::task::spawn_blocking` internally since rusqlite is sync.
    pub async fn open(db_path: &std::path::Path) -> StoreResult<Self>;

    /// In-memory database for tests (`rusqlite::Connection::open_in_memory`).
    pub async fn open_in_memory() -> StoreResult<Self>;
}

pub mod models {
    use chrono::{DateTime, Utc};

    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
    pub struct Conversation {
        pub id: String,          // uuid v4 string
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
        pub role: String,        // "system" | "user" | "assistant" | "tool"
        pub content: String,
        pub provider_id: Option<String>,
        pub model_id: Option<String>,
        pub created_at: DateTime<Utc>,
    }

    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
    pub struct Automation {
        pub id: String,
        pub name: String,
        /// Serialized `nexus_automation::AutomationDefinition` (owned by
        /// the automation crate; stored here as opaque JSON text).
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
        pub status: String,      // "running" | "success" | "failed" | "cancelled"
        pub log_json: String,    // serialized Vec<StepLogEntry>
    }

    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
    pub struct UsageEvent {
        pub id: String,
        pub occurred_at: DateTime<Utc>,
        pub provider_id: String,
        pub model_id: String,
        pub kind: String,        // "chat" | "cli" | "automation"
        pub input_tokens: u32,
        pub output_tokens: u32,
        pub latency_ms: u32,
        pub success: bool,
        pub error_message: Option<String>,
    }
}

pub mod conversations {
    use super::{models::Conversation, Store, StoreResult};
    pub async fn create(store: &Store, title: &str) -> StoreResult<Conversation>;
    pub async fn list(store: &Store, folder: Option<&str>) -> StoreResult<Vec<Conversation>>;
    pub async fn get(store: &Store, id: &str) -> StoreResult<Option<Conversation>>;
    pub async fn rename(store: &Store, id: &str, title: &str) -> StoreResult<()>;
    pub async fn set_favorite(store: &Store, id: &str, favorite: bool) -> StoreResult<()>;
    pub async fn delete(store: &Store, id: &str) -> StoreResult<()>;
    pub async fn search(store: &Store, query: &str) -> StoreResult<Vec<Conversation>>;
    pub async fn delete_all(store: &Store) -> StoreResult<()>;
}

pub mod messages {
    use super::{models::Message, Store, StoreResult};
    pub async fn append(store: &Store, conversation_id: &str, msg: Message) -> StoreResult<()>;
    pub async fn list_for_conversation(store: &Store, conversation_id: &str) -> StoreResult<Vec<Message>>;
}

pub mod automations {
    use super::{models::{Automation, AutomationRun}, Store, StoreResult};
    pub async fn upsert(store: &Store, automation: Automation) -> StoreResult<()>;
    pub async fn list(store: &Store) -> StoreResult<Vec<Automation>>;
    pub async fn get(store: &Store, id: &str) -> StoreResult<Option<Automation>>;
    pub async fn delete(store: &Store, id: &str) -> StoreResult<()>;
    pub async fn record_run(store: &Store, run: AutomationRun) -> StoreResult<()>;
    pub async fn list_runs(store: &Store, automation_id: &str, limit: u32) -> StoreResult<Vec<AutomationRun>>;
}

pub mod usage {
    use super::{models::UsageEvent, Store, StoreResult};
    use chrono::{DateTime, Utc};

    #[derive(Debug, Clone, Default)]
    pub struct UsageFilter {
        pub since: Option<DateTime<Utc>>,
        pub until: Option<DateTime<Utc>>,
        pub provider_id: Option<String>,
        pub model_id: Option<String>,
        pub kind: Option<String>,
    }

    pub async fn record(store: &Store, event: UsageEvent) -> StoreResult<()>;
    pub async fn query(store: &Store, filter: UsageFilter) -> StoreResult<Vec<UsageEvent>>;
}

pub mod settings {
    use super::{Store, StoreResult};
    pub async fn get(store: &Store, key: &str) -> StoreResult<Option<String>>;
    pub async fn set(store: &Store, key: &str, value: &str) -> StoreResult<()>;
}
```

Implementer notes for `nexus-store`:
- Embed schema as versioned SQL strings in `src/migrations.rs`, applied
  inside a `user_version` pragma check so `open()` is idempotent.
- All ids are `uuid::Uuid::new_v4().to_string()`.
- `rusqlite` is accessed only inside `spawn_blocking`; the public async
  functions above are the *only* way other crates touch the database.
- Add `#[cfg(test)]` tests using `Store::open_in_memory()` for every module.

## `nexus-automation` depends on

- `nexus_core::{Provider, CliAgent, ChatRequest, ChatMessage, ChatEvent}`
  for the "prompt a model" and "run a CLI" step kinds.
- `nexus_store::{Store, models::AutomationRun, automations::record_run}`
  for the run log — written by the engine after every run.
- A `ProviderRegistry`/`CliAgentRegistry` is **not** defined yet at the
  time this contract was frozen; the automation engine must accept
  `Arc<dyn Provider>` / `Arc<dyn CliAgent>` trait objects passed in by the
  caller (the GTK application wires concrete instances), not look them up
  by name itself. Keep the engine free of any provider-specific code.

## `nexus-stats` depends on

- `nexus_store::{Store, usage::{query, UsageFilter}, models::UsageEvent}`.
- A hand-maintained `pricing.toml` (see `data/pricing.toml`, owned by this
  crate's implementer) mapping `provider_id.model_id -> {input_per_million,
  output_per_million}` in USD. **Never invent a price**: if a model is not
  in the file, cost must be reported as "unknown", not zero or guessed.
