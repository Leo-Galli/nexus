//! Automation engine: a small step interpreter driving `Provider` and
//! `CliAgent` trait objects supplied by the caller, with a trigger
//! scheduler (manual / cron-like / folder-watch) and a run log persisted
//! through `nexus-store`.
//!
//! This crate deliberately knows nothing about *which* providers or CLI
//! agents exist — the GTK application constructs an `ExecutionContext`
//! with the concrete `Arc<dyn Provider>` / `Arc<dyn CliAgent>` instances
//! the user has configured, keyed by the binding name used inside the
//! automation definition.

pub mod engine;
pub mod error;
pub mod notifier;
pub mod schedule;
pub mod step;
pub mod template;
pub mod trigger;

pub use engine::{execute_automation, ExecutionContext, StepLogEntry};
pub use error::AutomationError;
pub use notifier::Notifier;
pub use step::{AutomationDefinition, Predicate, Step, StepConfig, TransformOp};
pub use trigger::Trigger;
