use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Trigger {
    Manual,
    /// Cron expression evaluated by the `cron` crate, which expects its
    /// own 6-field `sec min hour day-of-month month day-of-week` syntax
    /// (not classic 5-field Unix cron) — the UI layer must show the user
    /// an example in that exact format.
    Scheduled { cron_expression: String },
    /// Fires when any file under `path` is created, modified, or removed.
    FolderChanged { path: String },
}
