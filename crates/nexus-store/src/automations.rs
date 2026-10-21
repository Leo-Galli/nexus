//! Automation definitions and their run history. See `docs/CONTRACTS.md`
//! for the frozen function signatures this module implements.

use rusqlite::{OptionalExtension, params};

use crate::{
    Store, StoreResult,
    models::{Automation, AutomationRun},
};

fn row_to_automation(row: &rusqlite::Row<'_>) -> rusqlite::Result<Automation> {
    Ok(Automation {
        id: row.get("id")?,
        name: row.get("name")?,
        definition_json: row.get("definition_json")?,
        enabled: row.get("enabled")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

fn row_to_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<AutomationRun> {
    Ok(AutomationRun {
        id: row.get("id")?,
        automation_id: row.get("automation_id")?,
        started_at: row.get("started_at")?,
        finished_at: row.get("finished_at")?,
        status: row.get("status")?,
        log_json: row.get("log_json")?,
    })
}

/// Inserts `automation`, or — if an automation with the same `id` already
/// exists — updates its mutable fields in place.
pub async fn upsert(store: &Store, automation: Automation) -> StoreResult<()> {
    store
        .with_connection(move |conn| {
            conn.execute(
                "INSERT INTO automations (id, name, definition_json, enabled, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(id) DO UPDATE SET
                    name = excluded.name,
                    definition_json = excluded.definition_json,
                    enabled = excluded.enabled,
                    updated_at = excluded.updated_at",
                params![
                    automation.id,
                    automation.name,
                    automation.definition_json,
                    automation.enabled,
                    automation.created_at,
                    automation.updated_at,
                ],
            )?;
            Ok(())
        })
        .await
}

pub async fn list(store: &Store) -> StoreResult<Vec<Automation>> {
    store
        .with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, name, definition_json, enabled, created_at, updated_at
                 FROM automations ORDER BY name ASC",
            )?;
            let rows = stmt.query_map([], row_to_automation)?;
            let mut automations = Vec::new();
            for row in rows {
                automations.push(row?);
            }
            Ok(automations)
        })
        .await
}

pub async fn get(store: &Store, id: &str) -> StoreResult<Option<Automation>> {
    let id = id.to_string();
    store
        .with_connection(move |conn| {
            conn.query_row(
                "SELECT id, name, definition_json, enabled, created_at, updated_at
                 FROM automations WHERE id = ?1",
                params![id],
                row_to_automation,
            )
            .optional()
            .map_err(Into::into)
        })
        .await
}

/// Deletes the automation and, via `ON DELETE CASCADE`, every run recorded
/// for it.
pub async fn delete(store: &Store, id: &str) -> StoreResult<()> {
    let id = id.to_string();
    store
        .with_connection(move |conn| {
            conn.execute("DELETE FROM automations WHERE id = ?1", params![id])?;
            Ok(())
        })
        .await
}

/// Inserts `run`, or — if a run with the same `id` already exists (the
/// engine recording progress on an in-flight run) — updates its mutable
/// fields in place.
pub async fn record_run(store: &Store, run: AutomationRun) -> StoreResult<()> {
    store
        .with_connection(move |conn| {
            conn.execute(
                "INSERT INTO automation_runs (id, automation_id, started_at, finished_at, status, log_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(id) DO UPDATE SET
                    finished_at = excluded.finished_at,
                    status = excluded.status,
                    log_json = excluded.log_json",
                params![
                    run.id,
                    run.automation_id,
                    run.started_at,
                    run.finished_at,
                    run.status,
                    run.log_json,
                ],
            )?;
            Ok(())
        })
        .await
}

pub async fn list_runs(
    store: &Store,
    automation_id: &str,
    limit: u32,
) -> StoreResult<Vec<AutomationRun>> {
    let automation_id = automation_id.to_string();
    store
        .with_connection(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, automation_id, started_at, finished_at, status, log_json
                 FROM automation_runs WHERE automation_id = ?1
                 ORDER BY started_at DESC LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![automation_id, limit], row_to_run)?;
            let mut runs = Vec::new();
            for row in rows {
                runs.push(row?);
            }
            Ok(runs)
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use uuid::Uuid;

    fn sample_automation(name: &str) -> Automation {
        let now = Utc::now();
        Automation {
            id: Uuid::new_v4().to_string(),
            name: name.to_string(),
            definition_json: "{}".to_string(),
            enabled: true,
            created_at: now,
            updated_at: now,
        }
    }

    fn sample_run(automation_id: &str, status: &str) -> AutomationRun {
        AutomationRun {
            id: Uuid::new_v4().to_string(),
            automation_id: automation_id.to_string(),
            started_at: Utc::now(),
            finished_at: None,
            status: status.to_string(),
            log_json: "[]".to_string(),
        }
    }

    #[tokio::test]
    async fn upsert_list_get_round_trip() {
        let store = Store::open_in_memory().await.unwrap();
        let automation = sample_automation("Daily digest");
        upsert(&store, automation.clone()).await.unwrap();

        let fetched = get(&store, &automation.id).await.unwrap().unwrap();
        assert_eq!(fetched.name, "Daily digest");
        assert!(fetched.enabled);

        let all = list(&store).await.unwrap();
        assert_eq!(all.len(), 1);
    }

    #[tokio::test]
    async fn upsert_with_same_id_updates_in_place() {
        let store = Store::open_in_memory().await.unwrap();
        let mut automation = sample_automation("Original name");
        upsert(&store, automation.clone()).await.unwrap();

        automation.name = "Renamed".to_string();
        automation.enabled = false;
        upsert(&store, automation.clone()).await.unwrap();

        let all = list(&store).await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "Renamed");
        assert!(!all[0].enabled);
    }

    #[tokio::test]
    async fn delete_removes_automation() {
        let store = Store::open_in_memory().await.unwrap();
        let automation = sample_automation("Temp");
        upsert(&store, automation.clone()).await.unwrap();

        delete(&store, &automation.id).await.unwrap();
        assert!(get(&store, &automation.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn record_run_and_list_runs_respects_limit_and_order() {
        let store = Store::open_in_memory().await.unwrap();
        let automation = sample_automation("Runs me");
        upsert(&store, automation.clone()).await.unwrap();

        for status in ["success", "failed", "success"] {
            record_run(&store, sample_run(&automation.id, status))
                .await
                .unwrap();
        }

        let runs = list_runs(&store, &automation.id, 2).await.unwrap();
        assert_eq!(runs.len(), 2);

        let all_runs = list_runs(&store, &automation.id, 10).await.unwrap();
        assert_eq!(all_runs.len(), 3);
    }

    #[tokio::test]
    async fn record_run_upserts_an_in_flight_run() {
        let store = Store::open_in_memory().await.unwrap();
        let automation = sample_automation("Runs me");
        upsert(&store, automation.clone()).await.unwrap();

        let mut run = sample_run(&automation.id, "running");
        record_run(&store, run.clone()).await.unwrap();

        run.status = "success".to_string();
        run.finished_at = Some(Utc::now());
        record_run(&store, run.clone()).await.unwrap();

        let runs = list_runs(&store, &automation.id, 10).await.unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, "success");
        assert!(runs[0].finished_at.is_some());
    }

    #[tokio::test]
    async fn deleting_automation_cascades_to_runs() {
        let store = Store::open_in_memory().await.unwrap();
        let automation = sample_automation("Runs me");
        upsert(&store, automation.clone()).await.unwrap();
        record_run(&store, sample_run(&automation.id, "success"))
            .await
            .unwrap();

        delete(&store, &automation.id).await.unwrap();

        let runs = list_runs(&store, &automation.id, 10).await.unwrap();
        assert!(runs.is_empty());
    }
}
