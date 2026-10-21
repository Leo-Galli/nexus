//! Shared contracts for Nexus: the `Provider` and `CliAgent` traits, chat
//! and model types, error types, the `SecretStore` trait, and HTTP/URL
//! helpers used by every provider and CLI agent implementation.
//!
//! This crate has no GTK dependency so it can be unit-tested in isolation
//! and reused by a future CLI or TUI front-end.

pub mod cli_agent;
pub mod error;
pub mod http;
pub mod model;
pub mod provider;
pub mod redact;
pub mod secrets;

pub use cli_agent::{
    CliAgent, CliAgentInfo, CliDetectionResult, CliInvocation, CommandOutput, CommandRunner,
    LoginStatus,
};
pub use error::{ProviderError, ProviderResult};
pub use model::{
    ChatEvent, ChatMessage, ChatRequest, FinishReason, ModelInfo, Role, TokenUsage, ToolCall,
    ToolDefinition,
};
pub use provider::{AuthMethod, ChatStream, Provider, ProviderKind, ProviderStatus};
pub use secrets::{SecretKey, SecretStore};
