use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Composite key identifying one secret. Mirrors the Secret Service
/// attribute model: `provider` + `account` are stored as lookup
/// attributes, never as part of the secret value.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SecretKey {
    /// `ProviderKind::as_str()` value, e.g. `"openai"`.
    pub provider: String,
    /// Usually `"default"`; lets a provider have more than one stored
    /// credential (e.g. two Azure OpenAI deployments).
    pub account: String,
}

impl SecretKey {
    pub fn new(provider: impl Into<String>, account: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            account: account.into(),
        }
    }

    pub fn default_for(provider: impl Into<String>) -> Self {
        Self::new(provider, "default")
    }
}

/// Secret storage contract. The only real implementation
/// (`nexus-secrets::KeyringSecretStore`) talks to the Secret Service /
/// `org.freedesktop.portal.Secret` via the `oo7` crate. Nothing in Nexus
/// ever writes a key to a plain file.
#[async_trait]
pub trait SecretStore: Send + Sync {
    async fn set(&self, key: &SecretKey, value: &str) -> Result<(), SecretError>;
    async fn get(&self, key: &SecretKey) -> Result<Option<String>, SecretError>;
    async fn delete(&self, key: &SecretKey) -> Result<(), SecretError>;
    /// List the providers that currently have a stored secret, for the
    /// "connected services" preferences page. Never returns values.
    async fn list_configured_providers(&self) -> Result<Vec<String>, SecretError>;
}

#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    #[error("secret service unavailable: {0}")]
    Unavailable(String),
    #[error("secret service operation failed: {0}")]
    Backend(String),
}
