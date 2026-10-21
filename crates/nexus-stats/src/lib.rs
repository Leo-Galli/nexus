//! Local usage statistics: aggregation over `nexus_store::usage` events
//! plus a hand-maintained pricing table for cost estimates. Nothing here
//! ever leaves the device, and nothing here ever invents a price.

pub mod aggregate;
pub mod csv_export;
pub mod pricing;

pub use aggregate::{AggregatedRow, BucketGranularity, aggregate};
pub use pricing::{ModelPrice, PricingError, PricingTable};
