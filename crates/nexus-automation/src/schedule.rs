use std::path::Path;
use std::str::FromStr;
use std::time::Duration as StdDuration;

use chrono::{DateTime, Utc};
use cron::Schedule;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use crate::error::AutomationError;

/// Parse a cron expression and return the next time it fires strictly
/// after `after`. Returns `None` if the schedule has no further
/// occurrences (practically never happens for a valid recurring
/// expression, but `cron`'s iterator contract allows it).
pub fn next_occurrence(
    cron_expression: &str,
    after: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, AutomationError> {
    let schedule = Schedule::from_str(cron_expression)
        .map_err(|e| AutomationError::InvalidRegex(format!("invalid cron expression: {e}")))?;
    Ok(schedule.after(&after).next())
}

/// Blocks the calling (dedicated) thread watching `path` for changes and
/// invokes `on_change` for every filesystem event, until the returned
/// `notify::RecommendedWatcher` is dropped. Intended to be run inside a
/// `tokio::task::spawn_blocking` by the caller, with `on_change` sending
/// into an `async_channel`/`tokio::sync::mpsc` to hand control back to
/// async code.
pub fn watch_folder_blocking(
    path: &Path,
    mut on_change: impl FnMut() + Send + 'static,
) -> notify::Result<RecommendedWatcher> {
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if res.is_ok() {
            on_change();
        }
    })?;
    watcher.watch(path, RecursiveMode::Recursive)?;
    Ok(watcher)
}

/// Debounce window used by callers driving `watch_folder_blocking`
/// through a channel: multiple filesystem events from one save operation
/// are coalesced into a single automation run.
pub const FOLDER_WATCH_DEBOUNCE: StdDuration = StdDuration::from_millis(500);

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn computes_next_occurrence() {
        // Every minute, at second 0.
        let after = Utc.with_ymd_and_hms(2026, 1, 1, 10, 0, 30).unwrap();
        let next = next_occurrence("0 * * * * *", after).unwrap().unwrap();
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 1, 1, 10, 1, 0).unwrap());
    }

    #[test]
    fn rejects_invalid_expression() {
        assert!(next_occurrence("not a cron expression", Utc::now()).is_err());
    }
}
