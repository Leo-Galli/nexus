//! Local SQLite-backed storage for Nexus: conversations, messages,
//! automations and their run history, usage/cost events, and a small
//! key-value settings table.
//!
//! `Store` is a thin async wrapper around a single `rusqlite::Connection`.
//! `rusqlite` is entirely synchronous, so every statement in this crate
//! runs inside `tokio::task::spawn_blocking`; the `conversations`,
//! `messages`, `automations`, `usage` and `settings` modules are the only
//! supported way to touch the database — nothing outside this crate ever
//! sees a `rusqlite::Connection` directly.

use std::path::Path;
use std::sync::{Arc, Mutex};

use rusqlite::Connection;

pub mod automations;
pub mod conversations;
pub mod messages;
mod migrations;
pub mod models;
pub mod settings;
pub mod usage;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("serde error: {0}")]
    Serde(#[from] serde_json::Error),
}

pub type StoreResult<T> = Result<T, StoreError>;

/// Handle to the Nexus SQLite database.
///
/// `docs/CONTRACTS.md` documents the private field as "a single
/// `tokio::sync::Mutex<rusqlite::Connection>`", describing the *intent*
/// (one connection, one lock, held only for the duration of the blocking
/// SQL work). The field type below is the implementation this crate
/// actually chose to realize that intent correctly: an
/// `Arc<std::sync::Mutex<Connection>>`, locked with the synchronous
/// `std::sync::Mutex` only inside a `spawn_blocking` closure and never
/// across an `.await`. This is the field's only deviation from the
/// contract doc, it is private, and it does not change anything about the
/// public API documented there (see the implementer notes in
/// `docs/CONTRACTS.md`, which explicitly leave this field's type up to the
/// implementer).
pub struct Store {
    conn: Arc<Mutex<Connection>>,
}

impl Store {
    /// Opens (creating if needed) the SQLite file at `db_path` and runs all
    /// pending migrations before returning.
    pub async fn open(db_path: &Path) -> StoreResult<Self> {
        let path = db_path.to_path_buf();
        let conn = tokio::task::spawn_blocking(move || -> StoreResult<Connection> {
            let mut conn = Connection::open(path)?;
            conn.pragma_update(None, "foreign_keys", true)?;
            migrations::run_migrations(&mut conn)?;
            Ok(conn)
        })
        .await
        .expect("spawn_blocking task panicked")?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// In-memory database for tests.
    pub async fn open_in_memory() -> StoreResult<Self> {
        let conn = tokio::task::spawn_blocking(|| -> StoreResult<Connection> {
            let mut conn = Connection::open_in_memory()?;
            conn.pragma_update(None, "foreign_keys", true)?;
            migrations::run_migrations(&mut conn)?;
            Ok(conn)
        })
        .await
        .expect("spawn_blocking task panicked")?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Runs `f` against the underlying connection on a blocking thread.
    /// This is the single place in the crate that bridges async callers to
    /// synchronous `rusqlite` calls: the (synchronous) mutex is acquired
    /// and released entirely inside the `spawn_blocking` closure, so it is
    /// never held across an `.await`.
    pub(crate) async fn with_connection<F, T>(&self, f: F) -> StoreResult<T>
    where
        F: FnOnce(&Connection) -> StoreResult<T> + Send + 'static,
        T: Send + 'static,
    {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let guard = conn.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            f(&guard)
        })
        .await
        .expect("spawn_blocking task panicked")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn open_in_memory_runs_migrations_and_is_usable() {
        let store = Store::open_in_memory().await.unwrap();
        // A settings round trip exercises both a real table and the
        // foreign-keys-enabled connection without depending on any other
        // module's tests.
        settings::set(&store, "theme", "dark").await.unwrap();
        assert_eq!(
            settings::get(&store, "theme").await.unwrap(),
            Some("dark".to_string())
        );
    }

    /// Migrations must be safe to re-run: opening the same on-disk database
    /// twice should neither error out nor touch data written the first
    /// time around.
    #[tokio::test]
    async fn migrations_are_idempotent_across_reopen() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();

        {
            let store = Store::open(&path).await.unwrap();
            conversations::create(&store, "first open").await.unwrap();
        }

        // Reopening re-runs `run_migrations` against a database that is
        // already fully migrated; this must be a no-op, and previously
        // written data must still be there.
        let store = Store::open(&path).await.unwrap();
        let conversations = conversations::list(&store, None).await.unwrap();
        assert_eq!(conversations.len(), 1);
        assert_eq!(conversations[0].title, "first open");
    }
}
