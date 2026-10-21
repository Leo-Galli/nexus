use crate::aggregate::AggregatedRow;

/// Render aggregated rows as CSV text (UTF-8, comma-separated, CRLF line
/// endings per RFC 4180 as produced by the `csv` crate). `estimated_cost_usd`
/// is written as an empty field, not `0`, when the price is unknown.
pub fn rows_to_csv(rows: &[AggregatedRow]) -> Result<String, csv::Error> {
    let mut writer = csv::WriterBuilder::new().from_writer(Vec::new());
    writer.write_record([
        "bucket",
        "provider_id",
        "model_id",
        "kind",
        "request_count",
        "input_tokens",
        "output_tokens",
        "estimated_cost_usd",
        "avg_latency_ms",
        "error_count",
    ])?;
    for row in rows {
        writer.write_record(&[
            row.bucket.clone(),
            row.provider_id.clone(),
            row.model_id.clone(),
            row.kind.clone(),
            row.request_count.to_string(),
            row.input_tokens.to_string(),
            row.output_tokens.to_string(),
            row.estimated_cost_usd
                .map(|c| format!("{c:.6}"))
                .unwrap_or_default(),
            format!("{:.2}", row.avg_latency_ms),
            row.error_count.to_string(),
        ])?;
    }
    let bytes = writer.into_inner().map_err(|e| e.into_error())?;
    Ok(String::from_utf8(bytes).expect("csv writer only emits UTF-8 for UTF-8 input"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_header_and_rows() {
        let rows = vec![AggregatedRow {
            bucket: "2026-03-10".into(),
            provider_id: "openai".into(),
            model_id: "gpt-4o-mini".into(),
            kind: "chat".into(),
            request_count: 3,
            input_tokens: 300,
            output_tokens: 150,
            estimated_cost_usd: Some(0.6),
            avg_latency_ms: 210.0,
            error_count: 1,
        }];
        let csv = rows_to_csv(&rows).unwrap();
        assert!(csv.contains("bucket,provider_id,model_id"));
        assert!(csv.contains("openai,gpt-4o-mini,chat,3,300,150,0.600000,210.00,1"));
    }

    #[test]
    fn empty_cost_is_blank_not_zero() {
        let rows = vec![AggregatedRow {
            bucket: "2026-03-10".into(),
            provider_id: "openai".into(),
            model_id: "unknown-model".into(),
            kind: "chat".into(),
            request_count: 1,
            input_tokens: 10,
            output_tokens: 5,
            estimated_cost_usd: None,
            avg_latency_ms: 100.0,
            error_count: 0,
        }];
        let csv = rows_to_csv(&rows).unwrap();
        let data_line = csv.lines().nth(1).unwrap();
        let fields: Vec<&str> = data_line.split(',').collect();
        assert_eq!(fields[7], "");
    }
}
