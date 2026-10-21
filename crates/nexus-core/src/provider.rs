use async_trait::async_trait;
use futures_core::stream::BoxStream;
use serde::{Deserialize, Serialize};

use crate::error::ProviderResult;
use crate::model::{ChatEvent, ChatRequest, ModelInfo};

pub type ChatStream = BoxStream<'static, ProviderResult<ChatEvent>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    /// User pastes an API key from the provider's dashboard, stored only
    /// via `SecretStore`.
    ApiKey,
    /// Provider offers a first-party OAuth flow to third-party apps.
    OAuth,
    /// Local server / local model: no credential needed.
    None,
}

/// Stable identifier for a provider implementation, used as the primary
/// key in settings, the secret store, and `usage_events.provider_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderKind {
    OpenAi,
    Anthropic,
    GoogleGemini,
    Mistral,
    XaiGrok,
    DeepSeek,
    Groq,
    OpenRouter,
    Together,
    Perplexity,
    Cohere,
    QwenDashScope,
    AzureOpenAi,
    AmazonBedrock,
    OpenAiCompatibleCustom,
    Ollama,
    LmStudio,
    LlamaCppServer,
}

impl ProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::Anthropic => "anthropic",
            Self::GoogleGemini => "google-gemini",
            Self::Mistral => "mistral",
            Self::XaiGrok => "xai-grok",
            Self::DeepSeek => "deepseek",
            Self::Groq => "groq",
            Self::OpenRouter => "openrouter",
            Self::Together => "together",
            Self::Perplexity => "perplexity",
            Self::Cohere => "cohere",
            Self::QwenDashScope => "qwen-dashscope",
            Self::AzureOpenAi => "azure-openai",
            Self::AmazonBedrock => "amazon-bedrock",
            Self::OpenAiCompatibleCustom => "openai-compatible-custom",
            Self::Ollama => "ollama",
            Self::LmStudio => "lm-studio",
            Self::LlamaCppServer => "llama-cpp-server",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderStatus {
    /// `true` once the user has supplied whatever credential this
    /// provider needs (or needs none).
    pub configured: bool,
    /// `None` when reachability has not been probed yet.
    pub reachable: Option<bool>,
    /// e.g. an account email/org name, when the API exposes one.
    pub account_label: Option<String>,
    pub detail: Option<String>,
}

impl ProviderStatus {
    pub fn not_configured() -> Self {
        Self {
            configured: false,
            reachable: None,
            account_label: None,
            detail: None,
        }
    }
}

/// Common contract for every remote or local AI provider.
///
/// Implementations must not block the calling task: `reqwest` futures are
/// awaited directly, and any synchronous work (e.g. reading a secret) must
/// itself be async or run via `spawn_blocking` inside the implementation.
#[async_trait]
pub trait Provider: Send + Sync {
    fn kind(&self) -> ProviderKind;
    fn display_name(&self) -> &'static str;
    fn auth_method(&self) -> AuthMethod;

    /// Official page where the user can create an API key, shown next to
    /// the key-entry field in Preferences. `None` for providers that never
    /// need one (local servers, OAuth-only).
    fn api_key_url(&self) -> Option<&'static str>;

    /// Cheap check of whether credentials are present and, when possible,
    /// a lightweight reachability probe. Must not make a billed request.
    async fn status(&self) -> ProviderResult<ProviderStatus>;

    /// Fetch the model list from the provider's official models endpoint
    /// (or CLI, for CLI-backed providers). Callers are responsible for
    /// caching; this always goes to the network.
    async fn list_models(&self) -> ProviderResult<Vec<ModelInfo>>;

    /// Start a streamed chat completion. The returned stream yields
    /// `ChatEvent`s until `ChatEvent::Done` or an `Err`.
    async fn send_chat(&self, request: ChatRequest) -> ProviderResult<ChatStream>;
}
