//! Generic provider for any service that implements (or claims to
//! implement) the OpenAI Chat Completions wire format:
//! `POST {base_url}/chat/completions` and `GET {base_url}/models`, both
//! using `Authorization: Bearer <key>` auth. This is what "OpenAI-API
//! compatible" conventionally means, so the request/response shapes and
//! the streaming/SSE parsing are reused directly from `openai.rs` (see the
//! `pub(crate)` items imported below) rather than duplicated — they are
//! the same wire format by construction.
//!
//! Per-preset confidence notes (also repeated on each constructor's doc
//! comment):
//! - **Mistral**, **Groq**, **DeepSeek**, **OpenRouter**, **Together**,
//!   **xAI (Grok)**: all publish official docs describing an
//!   OpenAI-compatible `/v1/chat/completions` and `/v1/models` endpoint
//!   with Bearer-token auth at the base URLs used below. **VERIFIED**
//!   against each provider's own API reference as of this writing.
//! - **Perplexity**: publishes an OpenAI-compatible `/chat/completions`
//!   endpoint, but this implementation could not verify from training
//!   knowledge that Perplexity exposes a standard `/models` *listing*
//!   endpoint returning the usual `{"data": [...]}` shape. Rather than
//!   guess, `list_models()` for the Perplexity preset returns
//!   `ProviderError::Unsupported` pointing at Perplexity's model-list
//!   documentation. **UNVERIFIED** for `/models`; chat completions are
//!   VERIFIED.
//! - **Qwen (Alibaba DashScope)**: the `dashscope-intl.aliyuncs.com`
//!   OpenAI-compatible base URL is documented for international accounts;
//!   mainland China accounts are documented to need
//!   `dashscope.aliyuncs.com` (no `-intl`) instead. **VERIFIED** that both
//!   variants exist; which one a given account needs is account-dependent,
//!   so the custom-URL constructor should be used to override this preset
//!   when needed.
//! - For every preset, `list_models()` still defensively handles a non-2xx
//!   or unexpected-JSON-shape response by returning a clear
//!   `ProviderError::InvalidResponse` rather than panicking, in case a
//!   preset's actual behavior differs from what is documented here.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use url::Url;

use nexus_core::error::{ProviderError, ProviderResult};
use nexus_core::http::{build_local_http_client, validate_endpoint_url};
use nexus_core::model::{ChatRequest, ModelInfo};
use nexus_core::provider::{AuthMethod, ChatStream, Provider, ProviderKind, ProviderStatus};
use nexus_core::secrets::{SecretKey, SecretStore};

use super::openai::{
    OpenAiChatRequestBody, OpenAiMessage, OpenAiStreamOptions, OpenAiToolDef,
    build_openai_sse_stream, error_from_response, role_to_str,
};

const USER_AGENT: &str = "nexus-providers (openai-compatible)";

pub struct OpenAiCompatibleProvider {
    http: reqwest::Client,
    base_url: Url,
    /// `None` for an endpoint that needs no credential at all (e.g. a bare
    /// local server reachable without auth). All current presets need one.
    api_key: Option<SecretKey>,
    secrets: Arc<dyn SecretStore>,
    /// Leaked once at construction so it can satisfy
    /// `Provider::display_name`'s `&'static str` contract. Provider
    /// instances are created once per app session and live for its
    /// lifetime, so the one-time leak of a short name is an acceptable
    /// trade-off; avoid constructing many short-lived instances.
    display_name: &'static str,
    kind: ProviderKind,
    api_key_url: Option<&'static str>,
    /// `false` only for presets (currently just Perplexity) where this
    /// implementation could not verify a `/models` listing endpoint exists
    /// in the standard OpenAI shape.
    supports_model_listing: bool,
}

impl OpenAiCompatibleProvider {
    #[allow(clippy::too_many_arguments)]
    fn build(
        base_url: &str,
        api_key: Option<SecretKey>,
        secrets: Arc<dyn SecretStore>,
        display_name: impl Into<String>,
        kind: ProviderKind,
        api_key_url: Option<&'static str>,
        supports_model_listing: bool,
    ) -> ProviderResult<Self> {
        let base_url = validate_endpoint_url(base_url)?;
        let display_name: &'static str = Box::leak(display_name.into().into_boxed_str());
        Ok(Self {
            // `validate_endpoint_url` already restricts plain HTTP to
            // loopback hosts, so this provider's own client must not also
            // force `https_only` — the named presets are all HTTPS anyway,
            // but `custom()` must still be able to reach a local
            // self-hosted OpenAI-compatible server (e.g. a local vLLM
            // instance at `http://127.0.0.1:...`).
            http: build_local_http_client(USER_AGENT)?,
            base_url,
            api_key,
            secrets,
            display_name,
            kind,
            api_key_url,
            supports_model_listing,
        })
    }

    /// Mistral AI. Base URL: `https://api.mistral.ai/v1`.
    /// VERIFIED: documented OpenAI-compatible `/v1/chat/completions` and
    /// `/v1/models`, Bearer-token auth.
    pub fn mistral(secrets: Arc<dyn SecretStore>) -> ProviderResult<Self> {
        Self::build(
            "https://api.mistral.ai/v1",
            Some(SecretKey::default_for(ProviderKind::Mistral.as_str())),
            secrets,
            "Mistral",
            ProviderKind::Mistral,
            Some("https://console.mistral.ai/api-keys"),
            true,
        )
    }

    /// Groq. Base URL: `https://api.groq.com/openai/v1`.
    /// VERIFIED: documented OpenAI-compatible `/openai/v1/chat/completions`
    /// and `/openai/v1/models`, Bearer-token auth.
    pub fn groq(secrets: Arc<dyn SecretStore>) -> ProviderResult<Self> {
        Self::build(
            "https://api.groq.com/openai/v1",
            Some(SecretKey::default_for(ProviderKind::Groq.as_str())),
            secrets,
            "Groq",
            ProviderKind::Groq,
            Some("https://console.groq.com/keys"),
            true,
        )
    }

    /// DeepSeek. Base URL: `https://api.deepseek.com/v1`.
    /// VERIFIED: documented OpenAI-compatible `/v1/chat/completions` and
    /// `/v1/models`, Bearer-token auth.
    pub fn deepseek(secrets: Arc<dyn SecretStore>) -> ProviderResult<Self> {
        Self::build(
            "https://api.deepseek.com/v1",
            Some(SecretKey::default_for(ProviderKind::DeepSeek.as_str())),
            secrets,
            "DeepSeek",
            ProviderKind::DeepSeek,
            Some("https://platform.deepseek.com/api_keys"),
            true,
        )
    }

    /// OpenRouter. Base URL: `https://openrouter.ai/api/v1`.
    /// VERIFIED: documented OpenAI-compatible `/api/v1/chat/completions`
    /// and `/api/v1/models`, Bearer-token auth.
    pub fn openrouter(secrets: Arc<dyn SecretStore>) -> ProviderResult<Self> {
        Self::build(
            "https://openrouter.ai/api/v1",
            Some(SecretKey::default_for(ProviderKind::OpenRouter.as_str())),
            secrets,
            "OpenRouter",
            ProviderKind::OpenRouter,
            Some("https://openrouter.ai/keys"),
            true,
        )
    }

    /// Together AI. Base URL: `https://api.together.xyz/v1`.
    /// VERIFIED: documented OpenAI-compatible `/v1/chat/completions` and
    /// `/v1/models`, Bearer-token auth.
    pub fn together(secrets: Arc<dyn SecretStore>) -> ProviderResult<Self> {
        Self::build(
            "https://api.together.xyz/v1",
            Some(SecretKey::default_for(ProviderKind::Together.as_str())),
            secrets,
            "Together AI",
            ProviderKind::Together,
            Some("https://api.together.ai/settings/api-keys"),
            true,
        )
    }

    /// Perplexity. Base URL: `https://api.perplexity.ai`.
    /// VERIFIED: documented OpenAI-compatible `/chat/completions`,
    /// Bearer-token auth. **UNVERIFIED**: a standard `/models` listing
    /// endpoint in the usual `{"data": [...]}` shape could not be
    /// confirmed from training knowledge/official docs at the time this
    /// was written, so `list_models()` deliberately refuses rather than
    /// guessing the response shape — see its doc comment.
    pub fn perplexity(secrets: Arc<dyn SecretStore>) -> ProviderResult<Self> {
        Self::build(
            "https://api.perplexity.ai",
            Some(SecretKey::default_for(ProviderKind::Perplexity.as_str())),
            secrets,
            "Perplexity",
            ProviderKind::Perplexity,
            Some("https://www.perplexity.ai/settings/api"),
            false,
        )
    }

    /// xAI (Grok). Base URL: `https://api.x.ai/v1`.
    /// VERIFIED: documented OpenAI-compatible `/v1/chat/completions` and
    /// `/v1/models`, Bearer-token auth.
    pub fn xai_grok(secrets: Arc<dyn SecretStore>) -> ProviderResult<Self> {
        Self::build(
            "https://api.x.ai/v1",
            Some(SecretKey::default_for(ProviderKind::XaiGrok.as_str())),
            secrets,
            "xAI (Grok)",
            ProviderKind::XaiGrok,
            Some("https://console.x.ai"),
            true,
        )
    }

    /// Alibaba Cloud Qwen via DashScope's OpenAI-compatible mode. Base URL:
    /// `https://dashscope-intl.aliyuncs.com/compatible-mode/v1` (the
    /// international endpoint). VERIFIED that this endpoint and
    /// `https://dashscope.aliyuncs.com/compatible-mode/v1` (mainland China
    /// accounts) both exist and are documented as OpenAI-compatible; which
    /// one a given account must use depends on where the account was
    /// created. If this preset's endpoint returns an auth error for a
    /// mainland China account, use
    /// [`OpenAiCompatibleProvider::custom`] with the non-`-intl` base URL
    /// instead of this constructor.
    pub fn qwen_dashscope(secrets: Arc<dyn SecretStore>) -> ProviderResult<Self> {
        Self::build(
            "https://dashscope-intl.aliyuncs.com/compatible-mode/v1",
            Some(SecretKey::default_for(ProviderKind::QwenDashScope.as_str())),
            secrets,
            "Qwen (DashScope)",
            ProviderKind::QwenDashScope,
            Some("https://dashscope.console.aliyun.com/apiKey"),
            true,
        )
    }

    /// Fully user-supplied OpenAI-compatible endpoint (self-hosted server,
    /// a provider not covered by a named preset, etc). `api_key` is
    /// `None` for endpoints that need no credential.
    pub fn custom(
        base_url: &str,
        api_key: Option<SecretKey>,
        display_name: impl Into<String>,
        secrets: Arc<dyn SecretStore>,
    ) -> ProviderResult<Self> {
        Self::build(
            base_url,
            api_key,
            secrets,
            display_name,
            ProviderKind::OpenAiCompatibleCustom,
            None,
            true,
        )
    }

    fn endpoint(&self, suffix: &str) -> String {
        format!(
            "{}/{}",
            self.base_url.as_str().trim_end_matches('/'),
            suffix
        )
    }

    async fn resolve_api_key(&self) -> ProviderResult<Option<String>> {
        match &self.api_key {
            None => Ok(None),
            Some(key) => {
                let value = self
                    .secrets
                    .get(key)
                    .await
                    .map_err(|e| ProviderError::Secret(e.to_string()))?;
                value.map(Some).ok_or(ProviderError::NotConfigured)
            }
        }
    }
}

#[async_trait]
impl Provider for OpenAiCompatibleProvider {
    fn kind(&self) -> ProviderKind {
        self.kind
    }

    fn display_name(&self) -> &'static str {
        self.display_name
    }

    fn auth_method(&self) -> AuthMethod {
        if self.api_key.is_some() {
            AuthMethod::ApiKey
        } else {
            AuthMethod::None
        }
    }

    fn api_key_url(&self) -> Option<&'static str> {
        self.api_key_url
    }

    async fn status(&self) -> ProviderResult<ProviderStatus> {
        let key = match &self.api_key {
            None => None,
            Some(key) => {
                let value = self
                    .secrets
                    .get(key)
                    .await
                    .map_err(|e| ProviderError::Secret(e.to_string()))?;
                match value {
                    Some(v) => Some(v),
                    None => return Ok(ProviderStatus::not_configured()),
                }
            }
        };

        if !self.supports_model_listing {
            return Ok(ProviderStatus {
                configured: true,
                reachable: None,
                account_label: None,
                detail: Some(
                    "no documented lightweight reachability check available for this provider"
                        .to_string(),
                ),
            });
        }

        let mut req = self.http.get(self.endpoint("models"));
        if let Some(key) = &key {
            req = req.bearer_auth(key);
        }

        match req.send().await {
            Ok(resp) if resp.status().is_success() => Ok(ProviderStatus {
                configured: true,
                reachable: Some(true),
                account_label: None,
                detail: None,
            }),
            Ok(resp) => Ok(ProviderStatus {
                configured: true,
                reachable: Some(false),
                account_label: None,
                detail: Some(format!("HTTP {}", resp.status())),
            }),
            Err(e) => Ok(ProviderStatus {
                configured: true,
                reachable: Some(false),
                account_label: None,
                detail: Some(e.to_string()),
            }),
        }
    }

    async fn list_models(&self) -> ProviderResult<Vec<ModelInfo>> {
        if !self.supports_model_listing {
            return Err(ProviderError::Unsupported(format!(
                "{} does not have a confirmed /models listing endpoint; see the provider's own \
                 model-list documentation instead of relying on auto-discovery here.",
                self.display_name
            )));
        }

        let key = self.resolve_api_key().await?;
        let mut req = self.http.get(self.endpoint("models"));
        if let Some(key) = &key {
            req = req.bearer_auth(key);
        }

        let resp = req.send().await.map_err(ProviderError::Network)?;
        if !resp.status().is_success() {
            return Err(error_from_response(resp).await);
        }

        let body = resp.text().await.map_err(ProviderError::Network)?;
        let parsed: CompatibleModelsResponse = serde_json::from_str(&body).map_err(|e| {
            ProviderError::InvalidResponse(format!(
                "{} returned an unexpected /models response shape: {e}",
                self.display_name
            ))
        })?;

        Ok(parsed
            .data
            .into_iter()
            .map(|entry| ModelInfo {
                id: entry.id.clone(),
                display_name: entry.id,
                context_window: None,
                max_output_tokens: None,
                supports_tools: false,
                supports_vision: false,
                input_price_per_million: None,
                output_price_per_million: None,
            })
            .collect())
    }

    async fn send_chat(&self, request: ChatRequest) -> ProviderResult<ChatStream> {
        let key = self.resolve_api_key().await?;

        let mut messages = Vec::new();
        if let Some(system) = &request.system_prompt
            && !system.is_empty()
        {
            messages.push(OpenAiMessage {
                role: "system".to_string(),
                content: system.clone(),
                tool_call_id: None,
            });
        }
        for m in &request.messages {
            messages.push(OpenAiMessage {
                role: role_to_str(m.role).to_string(),
                content: m.content.clone(),
                tool_call_id: m.tool_call_id.clone(),
            });
        }

        let tools: Vec<OpenAiToolDef> = request
            .tools
            .iter()
            .map(OpenAiToolDef::from_definition)
            .collect();

        let body = OpenAiChatRequestBody {
            model: &request.model_id,
            messages,
            stream: true,
            stream_options: OpenAiStreamOptions {
                include_usage: true,
            },
            temperature: request.temperature,
            max_tokens: request.max_tokens,
            tools,
        };

        let mut req = self
            .http
            .post(self.endpoint("chat/completions"))
            .json(&body);
        if let Some(key) = &key {
            req = req.bearer_auth(key);
        }

        let resp = req.send().await.map_err(ProviderError::Network)?;
        if !resp.status().is_success() {
            return Err(error_from_response(resp).await);
        }

        Ok(build_openai_sse_stream(resp))
    }
}

#[derive(Debug, Deserialize)]
struct CompatibleModelsResponse {
    #[serde(default)]
    data: Vec<CompatibleModelEntry>,
}

#[derive(Debug, Deserialize)]
struct CompatibleModelEntry {
    id: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait as async_trait_test;
    use futures_util::StreamExt;
    use nexus_core::error::ProviderError;
    use nexus_core::model::{ChatEvent, FinishReason, TokenUsage};
    use nexus_core::secrets::SecretError;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    struct FakeSecretStore {
        key: Option<String>,
    }

    #[async_trait_test]
    impl SecretStore for FakeSecretStore {
        async fn set(&self, _key: &SecretKey, _value: &str) -> Result<(), SecretError> {
            Ok(())
        }
        async fn get(&self, _key: &SecretKey) -> Result<Option<String>, SecretError> {
            Ok(self.key.clone())
        }
        async fn delete(&self, _key: &SecretKey) -> Result<(), SecretError> {
            Ok(())
        }
        async fn list_configured_providers(&self) -> Result<Vec<String>, SecretError> {
            Ok(Vec::new())
        }
    }

    fn custom_provider(base_url: String, key: &str) -> OpenAiCompatibleProvider {
        let secrets = Arc::new(FakeSecretStore {
            key: Some(key.to_string()),
        });
        OpenAiCompatibleProvider::custom(
            &base_url,
            Some(SecretKey::new("test", "default")),
            "Test",
            secrets,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn list_models_parses_fixture() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .and(header("authorization", "Bearer test-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{"id": "llama-3.1-70b", "object": "model"}]
            })))
            .mount(&server)
            .await;

        let provider = custom_provider(server.uri(), "test-key");
        let models = provider.list_models().await.unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "llama-3.1-70b");
    }

    #[tokio::test]
    async fn send_chat_parses_sse_stream() {
        let server = MockServer::start().await;
        let sse_body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":4,\"completion_tokens\":1}}\n\n",
            "data: [DONE]\n\n",
        );

        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_raw(sse_body, "text/event-stream"),
            )
            .mount(&server)
            .await;

        let provider = custom_provider(server.uri(), "test-key");
        let request = ChatRequest {
            model_id: "llama-3.1-70b".into(),
            messages: vec![nexus_core::model::ChatMessage::user("hi")],
            system_prompt: None,
            temperature: None,
            max_tokens: None,
            tools: Vec::new(),
            stream: true,
        };

        let events: Vec<_> = provider
            .send_chat(request)
            .await
            .unwrap()
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();

        assert_eq!(events.len(), 3);
        assert!(matches!(&events[0], ChatEvent::TextDelta(s) if s == "Hi"));
        assert!(matches!(
            &events[1],
            ChatEvent::Usage(TokenUsage {
                input_tokens: 4,
                output_tokens: 1
            })
        ));
        assert!(matches!(
            &events[2],
            ChatEvent::Done {
                finish_reason: FinishReason::Stop
            }
        ));
    }

    #[tokio::test]
    async fn unauthorized_maps_to_auth_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
                "error": "invalid api key"
            })))
            .mount(&server)
            .await;

        let provider = custom_provider(server.uri(), "bad-key");
        let err = provider.list_models().await.unwrap_err();
        assert!(matches!(err, ProviderError::Auth(_)));
    }

    #[tokio::test]
    async fn perplexity_preset_refuses_model_listing() {
        let secrets = Arc::new(FakeSecretStore {
            key: Some("k".to_string()),
        });
        let provider = OpenAiCompatibleProvider::perplexity(secrets).unwrap();
        let err = provider.list_models().await.unwrap_err();
        assert!(matches!(err, ProviderError::Unsupported(_)));
    }
}
