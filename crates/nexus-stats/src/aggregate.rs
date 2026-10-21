use std::collections::BTreeMap;

use chrono::{DateTime, Datelike, Utc};
use nexus_store::{Store, StoreResult, usage};
use serde::Serialize;

use crate::pricing::PricingTable;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BucketGranularity {
    Day,
    Week,
    Month,
}

fn bucket_key(dt: DateTime<Utc>, granularity: BucketGranularity) -> String {
    match granularity {
        BucketGranularity::Day => dt.format("%Y-%m-%d").to_string(),
        BucketGranularity::Week => {
            let week = dt.iso_week();
            format!("{}-W{:02}", week.year(), week.week())
        }
        BucketGranularity::Month => dt.format("%Y-%m").to_string(),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AggregatedRow {
    pub bucket: String,
    pub provider_id: String,
    pub model_id: String,
    pub kind: String,
    pub request_count: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// `None` when the model is not in the pricing table — never a
    /// guessed value, see `data/pricing.toml`.
    pub estimated_cost_usd: Option<f64>,
    pub avg_latency_ms: f64,
    pub error_count: u32,
}

#[derive(Default)]
struct Accumulator {
    request_count: u32,
    input_tokens: u64,
    output_tokens: u64,
    latency_sum_ms: u64,
    error_count: u32,
}

/// Group matching `usage_events` by (time bucket, provider, model, kind)
/// and compute per-group totals plus a cost estimate from `pricing`.
pub async fn aggregate(
    store: &Store,
    filter: usage::UsageFilter,
    granularity: BucketGranularity,
    pricing: &PricingTable,
) -> StoreResult<Vec<AggregatedRow>> {
    let events = usage::query(store, filter).await?;

    let mut groups: BTreeMap<(String, String, String, String), Accumulator> = BTreeMap::new();
    for event in &events {
        let key = (
            bucket_key(event.occurred_at, granularity),
            event.provider_id.clone(),
            event.model_id.clone(),
            event.kind.clone(),
        );
        let acc = groups.entry(key).or_default();
        acc.request_count += 1;
        acc.input_tokens += event.input_tokens as u64;
        acc.output_tokens += event.output_tokens as u64;
        acc.latency_sum_ms += event.latency_ms as u64;
        if !event.success {
            acc.error_count += 1;
        }
    }

    let rows = groups
        .into_iter()
        .map(|((bucket, provider_id, model_id, kind), acc)| {
            let estimated_cost_usd = pricing.estimate_cost_usd(
                &provider_id,
                &model_id,
                acc.input_tokens,
                acc.output_tokens,
            );
            let avg_latency_ms = if acc.request_count > 0 {
                acc.latency_sum_ms as f64 / acc.request_count as f64
            } else {
                0.0
            };
            AggregatedRow {
                bucket,
                provider_id,
                model_id,
                kind,
                request_count: acc.request_count,
                input_tokens: acc.input_tokens,
                output_tokens: acc.output_tokens,
                estimated_cost_usd,
                avg_latency_ms,
                error_count: acc.error_count,
            }
        })
        .collect();
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use nexus_store::models::UsageEvent;

    async fn seed(store: &Store) {
        let base = Utc.with_ymd_and_hms(2026, 3, 10, 12, 0, 0).unwrap();
        for i in 0..3u32 {
            usage::record(
                store,
                UsageEvent {
                    id: format!("evt-{i}"),
                    occurred_at: base,
                    provider_id: "openai".into(),
                    model_id: "gpt-4o-mini".into(),
                    kind: "chat".into(),
                    input_tokens: 100,
                    output_tokens: 50,
                    latency_ms: 200 + i * 10,
                    success: i != 2,
                    error_message: if i == 2 { Some("boom".into()) } else { None },
                },
            )
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn aggregates_by_day_with_known_price() {
        let store = Store::open_in_memory().await.unwrap();
        seed(&store).await;
        let mut pricing_toml = String::new();
        pricing_toml.push_str(
            "[openai.\"gpt-4o-mini\"]\ninput_per_million_usd = 1.0\noutput_per_million_usd = 2.0\n",
        );
        let pricing = PricingTable::from_toml_str(&pricing_toml).unwrap();

        let rows = aggregate(
            &store,
            usage::UsageFilter::default(),
            BucketGranularity::Day,
            &pricing,
        )
        .await
        .unwrap();

        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.request_count, 3);
        assert_eq!(row.input_tokens, 300);
        assert_eq!(row.output_tokens, 150);
        assert_eq!(row.error_count, 1);
        assert!(row.estimated_cost_usd.is_some());
    }

    #[tokio::test]
    async fn unknown_model_price_is_none() {
        let store = Store::open_in_memory().await.unwrap();
        seed(&store).await;
        let pricing = PricingTable::from_toml_str("").unwrap();

        let rows = aggregate(
            &store,
            usage::UsageFilter::default(),
            BucketGranularity::Day,
            &pricing,
        )
        .await
        .unwrap();
        assert_eq!(rows[0].estimated_cost_usd, None);
    }
}
