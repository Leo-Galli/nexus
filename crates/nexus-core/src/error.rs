use thiserror::Error;

/// Error type shared by every `Provider` implementation.
///
/// Variants are deliberately coarse: UI code maps them to a handful of
/// user-facing states (toast, retry banner, "open API key page" action)
/// rather than branching on provider-specific detail.
#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("authentication failed: {0}")]
    Auth(String),

    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("the server returned an unexpected response: {0}")]
    InvalidResponse(String),

    #[error("rate limited{}", .retry_after_secs.map(|s| format!(", retry after {s}s")).unwrap_or_default())]
    RateLimited { retry_after_secs: Option<u64> },

    #[error("model not found: {0}")]
    ModelNotFound(String),

    #[error("this provider is not configured yet")]
    NotConfigured,

    #[error("unsupported operation: {0}")]
    Unsupported(String),

    #[error("failed to (de)serialize data: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("the request was cancelled")]
    Cancelled,

    #[error("secret storage error: {0}")]
    Secret(String),

    #[error("local I/O error: {0}")]
    Io(#[from] std::io::Error),
}

pub type ProviderResult<T> = Result<T, ProviderError>;
