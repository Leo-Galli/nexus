//! One module per `Provider` implementation. Every module exposes a
//! `pub struct ...Provider` with its own `new(...)` constructor; the GTK
//! application decides which ones to instantiate based on what the user
//! has configured (see `nexus_store::settings` for saved provider config
//! and `nexus_core::SecretStore` for credentials). There is deliberately
//! no provider registry/factory in this crate: construction parameters
//! differ too much (API key vs. none vs. base URL) to make one useful.

pub mod providers;

pub use providers::*;
