//! Usage/cost event recording and filtered queries. See
//! `docs/CONTRACTS.md` for the frozen function signatures this module
//! implements.

use chrono::{DateTime, Utc};
use rusqlite::types::ToSql;
use rusqlite::params;

use crate::{models::UsageEvent, Store, StoreResult};

#[derive(Debug, Clone, Default)]
pub struct UsageFilter {
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    pub provider_id: Option<String>,
    pub model_id: Option<String>,
    pub kind: Option<String>,
}

fn row_to_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<UsageEvent> {
    Ok(UsageEvent {
        id: row.get("id")?,
        occurred_at: row.get("occurred_at")?,
        provider_id: row.get("provider_id")?,
        model_id: row.get("model_id")?,
        kind: row.get("kind")?,
        input_tokens: row.get("input_tokens")?,
        output_tokens: row.get("output_tokens")?,
        latency_ms: row.get("latency_ms")?,
        success: row.get("success")?,
        error_message: row.get("error_message")?,
    })
}

pub async fn record(store: &Store, event: UsageEvent) -> StoreResult<()> {
    store
        .with_connection(move |conn| {
            conn.execute(
                "INSERT INTO usage_events
                    (id, occurred_at, provider_id, model_id, kind, input_tokens, output_tokens, latency_ms, success, error_message)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    event.id,
                    event.occurred_at,
                    event.provider_id,
                    event.model_id,
                    event.kind,
                    event.input_tokens,
                    event.output_tokens,
                    event.latency_ms,
                    event.success,
                    event.error_message,
                ],
            )?;
            Ok(())
        })
        .await
}

/// Builds a dynamic `WHERE` clause from whichever `filter` fields are
/// `Some`. Only column names/operators are ever interpolated into the SQL
/// string; every value from `filter` is passed as a bound parameter.
pub async fn query(store: &Store, filter: UsageFilter) -> StoreResult<Vec<UsageEvent>> {
    store
        .with_connection(move |conn| {
            let mut clauses: Vec<&str> = Vec::new();
            let mut bound: Vec<Box<dyn ToSql>> = Vec::new();

            if let Some(since) = filter.since {
                clauses.push("occurred_at >= ?");
                bound.push(Box::new(since));
            }
            if let Some(until) = filter.until {
                clauses.push("occurred_at <= ?");
                bound.push(Box::new(until));
            }
            if let Some(provider_id) = filter.provider_id {
                clauses.push("provider_id = ?");
                bound.push(Box::new(provider_id));
            }
            if let Some(model_id) = filter.model_id {
                clauses.push("model_id = ?");
                bound.push(Box::new(model_id));
            }
            if let Some(kind) = filter.kind {
                clauses.push("kind = ?");
                bound.push(Box::new(kind));
            }

            let mut sql = "SELECT id, occurred_at, provider_id, model_id, kind, input_tokens, output_tokens, latency_ms, success, error_message
                           FROM usage_events"
                .to_string();
            if !clauses.is_empty() {
                sql.push_str(" WHERE ");
                sql.push_str(&clauses.join(" AND "));
            }
            sql.push_str(" ORDER BY occurred_at DESC");

            let mut stmt = conn.prepare(&sql)?;
            let param_refs: Vec<&dyn ToSql> = bound.iter().map(|b| b.as_ref()).collect();
            let rows = stmt.query_map(param_refs.as_slice(), row_to_event)?;

            let mut events = Vec::new();
            for row in rows {
                events.push(row?);
            }
            Ok(events)
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};
    use uuid::Uuid;

    fn sample_event(provider_id: &str, model_id: &str, kind: &str, occurred_at: DateTime<Utc>) -> UsageEvent {
        UsageEvent {
            id: Uuid::new_v4().to_string(),
            occurred_at,
            provider_id: provider_id.to_string(),
            model_id: model_id.to_string(),
            kind: kind.to_string(),
            input_tokens: 100,
            output_tokens: 50,
            latency_ms: 250,
            success: true,
            error_message: None,
        }
    }

    async fn seed(store: &Store) -> DateTime<Utc> {
        let now = Utc::now();
        record(store, sample_event("openai", "gpt-5", "chat", now - Duration::hours(2)))
            .await
            .unwrap();
        record(
            store,
            sample_event("anthropic", "claude-sonnet-5", "cli", now - Duration::hours(1)),
        )
        .await
        .unwrap();
        record(store, sample_event("openai", "gpt-5", "automation", now))
            .await
            .unwrap();
        now
    }

    #[tokio::test]
    async fn record_and_query_with_no_filter_returns_everything() {
        let store = Store::open_in_memory().await.unwrap();
        seed(&store).await;

        let events = query(&store, UsageFilter::default()).await.unwrap();
        assert_eq!(events.len(), 3);
        // Ordered most-recent first.
        assert_eq!(events[0].kind, "automation");
    }

    #[tokio::test]
    async fn filter_by_provider_id() {
        let store = Store::open_in_memory().await.unwrap();
        seed(&store).await;

        let events = query(
            &store,
            UsageFilter {
                provider_id: Some("openai".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        assert_eq!(events.len(), 2);
        assert!(events.iter().all(|e| e.provider_id == "openai"));
    }

    #[tokio::test]
    async fn filter_by_kind_and_model_id_combined() {
        let store = Store::open_in_memory().await.unwrap();
        seed(&store).await;

        let events = query(
            &store,
            UsageFilter {
                kind: Some("chat".to_string()),
                model_id: Some("gpt-5".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "chat");
    }

    #[tokio::test]
    async fn filter_by_since_and_until_range() {
        let store = Store::open_in_memory().await.unwrap();
        let now = seed(&store).await;

        let events = query(
            &store,
            UsageFilter {
                since: Some(now - Duration::minutes(90)),
                until: Some(now - Duration::minutes(30)),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "cli");
    }

    #[tokio::test]
    async fn filter_matching_nothing_returns_empty() {
        let store = Store::open_in_memory().await.unwrap();
        seed(&store).await;

        let events = query(
            &store,
            UsageFilter {
                provider_id: Some("does-not-exist".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        assert!(events.is_empty());
    }
}
