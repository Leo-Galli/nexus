use std::collections::HashMap;

use serde::Deserialize;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct ModelPrice {
    pub input_per_million_usd: f64,
    pub output_per_million_usd: f64,
}

/// `provider_id -> model_id -> price`. Deserialized from `data/pricing.toml`
/// (shipped empty — see that file's header comment). Never falls back to
/// a guessed price: `lookup` returns `None` for anything not listed.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PricingTable(HashMap<String, HashMap<String, ModelPrice>>);

#[derive(Debug, Error)]
pub enum PricingError {
    #[error("invalid pricing table: {0}")]
    Toml(#[from] toml::de::Error),
}

impl PricingTable {
    pub fn from_toml_str(s: &str) -> Result<Self, PricingError> {
        Ok(toml::from_str(s)?)
    }

    /// The pricing table Nexus ships with, embedded at compile time from
    /// `data/pricing.toml`. Maintainers update that file; the running
    /// application also supports loading a user-edited copy from the
    /// config directory (wired up by the GTK application, not here).
    pub fn bundled() -> Self {
        Self::from_toml_str(include_str!("../../../data/pricing.toml"))
            .expect("bundled data/pricing.toml must be valid TOML")
    }

    pub fn lookup(&self, provider_id: &str, model_id: &str) -> Option<ModelPrice> {
        self.0.get(provider_id)?.get(model_id).copied()
    }

    pub fn estimate_cost_usd(
        &self,
        provider_id: &str,
        model_id: &str,
        input_tokens: u64,
        output_tokens: u64,
    ) -> Option<f64> {
        let price = self.lookup(provider_id, model_id)?;
        Some(
            (input_tokens as f64 / 1_000_000.0) * price.input_per_million_usd
                + (output_tokens as f64 / 1_000_000.0) * price.output_per_million_usd,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_table_parses() {
        // The shipped file is empty of entries but must still be valid TOML.
        let table = PricingTable::bundled();
        assert_eq!(table.lookup("openai", "gpt-4o"), None);
    }

    #[test]
    fn looks_up_known_price() {
        let toml = r#"
            [openai."gpt-4o-mini"]
            input_per_million_usd = 0.15
            output_per_million_usd = 0.60
        "#;
        let table = PricingTable::from_toml_str(toml).unwrap();
        let price = table.lookup("openai", "gpt-4o-mini").unwrap();
        assert_eq!(price.input_per_million_usd, 0.15);
    }

    #[test]
    fn unknown_model_is_none_not_zero() {
        let table = PricingTable::from_toml_str("").unwrap();
        assert_eq!(
            table.estimate_cost_usd("openai", "does-not-exist", 1000, 1000),
            None
        );
    }

    #[test]
    fn estimates_cost_from_known_price() {
        let toml = r#"
            [openai."gpt-4o-mini"]
            input_per_million_usd = 1.0
            output_per_million_usd = 2.0
        "#;
        let table = PricingTable::from_toml_str(toml).unwrap();
        let cost = table
            .estimate_cost_usd("openai", "gpt-4o-mini", 1_000_000, 500_000)
            .unwrap();
        assert!((cost - 2.0).abs() < 1e-9);
    }
}
