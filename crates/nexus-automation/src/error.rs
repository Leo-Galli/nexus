use thiserror::Error;

#[derive(Debug, Error)]
pub enum AutomationError {
    #[error("unknown provider binding: {0}")]
    UnknownProvider(String),
    #[error("unknown CLI agent binding: {0}")]
    UnknownCliAgent(String),
    #[error("provider error: {0}")]
    Provider(#[from] nexus_core::ProviderError),
    #[error("CLI invocation error: {0}")]
    CliUnsupported(String),
    #[error("CLI process error: {0}")]
    CliIo(#[from] std::io::Error),
    #[error("CLI command exited with a non-zero status: {0}")]
    CliFailed(String),
    #[error("unknown template variable: {0}")]
    UnknownVariable(String),
    #[error("invalid regular expression in step: {0}")]
    InvalidRegex(String),
    #[error("store error: {0}")]
    Store(#[from] nexus_store::StoreError),
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
}
