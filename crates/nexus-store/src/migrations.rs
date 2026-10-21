//! Versioned schema migrations, tracked via SQLite's `PRAGMA user_version`.
//!
//! Each entry in [`MIGRATIONS`] is the complete SQL for one migration step,
//! applied inside its own transaction. The (1-based) position of a
//! migration in the array is the `user_version` the database has *after*
//! that migration runs, so bringing a database up to date is just "run
//! every migration whose index is >= the current `user_version`". This
//! makes `run_migrations` idempotent: calling it again on an
//! already-migrated database runs zero migrations.
//!
//! Schema notes:
//! - `DateTime<Utc>` columns are stored as RFC3339 `TEXT` (handled
//!   transparently by `rusqlite`'s `chrono` feature on both the write and
//!   read side).
//! - Booleans are stored as `INTEGER` 0/1 (also handled transparently by
//!   `rusqlite`).
//! - `messages.conversation_id` and `automation_runs.automation_id` are
//!   foreign keys with `ON DELETE CASCADE`, so deleting a conversation or
//!   automation cleans up its children automatically — enforcement
//!   requires `PRAGMA foreign_keys = ON`, which `Store::open`/
//!   `Store::open_in_memory` set on every connection they create.

use rusqlite::Connection;

use crate::StoreResult;

/// One SQL string per migration step, in order. Append new steps to the
/// end; never edit or reorder a step that has already shipped.
pub(crate) const MIGRATIONS: &[&str] = &[
    // --- migration 1: initial schema ---------------------------------
    r#"
    CREATE TABLE conversations (
        id          TEXT PRIMARY KEY,
        title       TEXT NOT NULL,
        folder      TEXT,
        is_favorite INTEGER NOT NULL DEFAULT 0,
        created_at  TEXT NOT NULL,
        updated_at  TEXT NOT NULL
    );

    CREATE INDEX idx_conversations_folder ON conversations(folder);

    CREATE TABLE messages (
        id              TEXT PRIMARY KEY,
        conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
        role            TEXT NOT NULL,
        content         TEXT NOT NULL,
        provider_id     TEXT,
        model_id        TEXT,
        created_at      TEXT NOT NULL
    );

    CREATE INDEX idx_messages_conversation_id ON messages(conversation_id);

    CREATE TABLE automations (
        id              TEXT PRIMARY KEY,
        name            TEXT NOT NULL,
        definition_json TEXT NOT NULL,
        enabled         INTEGER NOT NULL DEFAULT 1,
        created_at      TEXT NOT NULL,
        updated_at      TEXT NOT NULL
    );

    CREATE TABLE automation_runs (
        id            TEXT PRIMARY KEY,
        automation_id TEXT NOT NULL REFERENCES automations(id) ON DELETE CASCADE,
        started_at    TEXT NOT NULL,
        finished_at   TEXT,
        status        TEXT NOT NULL,
        log_json      TEXT NOT NULL
    );

    CREATE INDEX idx_automation_runs_automation_id ON automation_runs(automation_id);

    CREATE TABLE usage_events (
        id            TEXT PRIMARY KEY,
        occurred_at   TEXT NOT NULL,
        provider_id   TEXT NOT NULL,
        model_id      TEXT NOT NULL,
        kind          TEXT NOT NULL,
        input_tokens  INTEGER NOT NULL,
        output_tokens INTEGER NOT NULL,
        latency_ms    INTEGER NOT NULL,
        success       INTEGER NOT NULL,
        error_message TEXT
    );

    CREATE INDEX idx_usage_events_occurred_at ON usage_events(occurred_at);

    CREATE TABLE settings (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
    "#,
];

/// Brings `conn` up to the latest schema version described by
/// [`MIGRATIONS`]. Safe to call every time a [`crate::Store`] is opened,
/// including against a database that is already fully migrated.
pub(crate) fn run_migrations(conn: &mut Connection) -> StoreResult<()> {
    let current_version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let current_version = usize::try_from(current_version).unwrap_or(0);

    for (index, migration) in MIGRATIONS.iter().enumerate().skip(current_version) {
        let tx = conn.transaction()?;
        tx.execute_batch(migration)?;
        let new_version = index + 1;
        // `PRAGMA user_version` does not accept bound parameters; the value
        // formatted in here is always our own trusted array index, never
        // caller-supplied data.
        tx.execute_batch(&format!("PRAGMA user_version = {new_version}"))?;
        tx.commit()?;
    }

    Ok(())
}
