//! `AzureOpenAiProvider`: Azure OpenAI Service
//! (<https://learn.microsoft.com/azure/ai-services/openai/>).
//!
//! Azure OpenAI is unlike every other provider in this crate in three
//! ways, all VERIFIED against Azure's documented API shape:
//!
//! 1. There is no single base URL: each customer provisions their own
//!    Azure resource, reachable at `https://<resource-name>.openai.azure.com`.
//! 2. The credential goes in a header literally named `api-key`, not
//!    `Authorization: Bearer`.
//! 3. The path segment that looks like a model id is actually the
//!    customer's **deployment name** — a label the customer chose when
//!    they deployed a specific base model under their resource. The
//!    request URL is
//!    `{resource_base_url}/openai/deployments/{deployment_name}/chat/completions?api-version=...`.
//!
//! The JSON request/response bodies otherwise mirror OpenAI's
//! `/chat/completions` SSE shape, which this module parses with its own
//! small, self-contained reader (duplicated rather than shared, so this
//! file compiles and tests standalone against `nexus-core` only).

use std::sync::Arc;

use async_trait::async_trait;
use futures_util::StreamExt;
use serde::Deserialize;
use url::Url;

use nexus_core::http::{build_http_client, validate_endpoint_url};
use nexus_core::{
    AuthMethod, ChatEvent, ChatRequest, ChatStream, FinishReason, ModelInfo, Provider,
    ProviderError, ProviderKind, ProviderResult, ProviderStatus, Role, SecretKey, SecretStore,
    TokenUsage,
};

const USER_AGENT: &str = concat!(
    "nexus-providers/",
    env!("CARGO_PKG_VERSION"),
    " (azure-openai)"
);

/// Default Azure OpenAI REST API version.
///
/// UNVERIFIED-TO-STAY-CURRENT: Azure revs this date-stamped string
/// periodically (new versions add features, and old ones are eventually
/// retired); whatever value is hardcoded here *will* go stale. This is
/// only a fallback for a brand-new setup — the Preferences UI must let the
/// user override it per deployment, and that override must always win
/// over this constant.
const DEFAULT_API_VERSION: &str = "2024-10-21";

pub struct AzureOpenAiProvider {
    client: reqwest::Client,
    secrets: Arc<dyn SecretStore>,
    /// The customer's own Azure resource endpoint, e.g.
    /// `https://my-resource.openai.azure.com/`.
    resource_base_url: Url,
    /// The customer's deployment name (NOT a model id) — see module docs.
    deployment_name: String,
    api_version: String,
}

impl AzureOpenAiProvider {
    /// `resource_base_url_raw`: the customer's own resource endpoint, as
    /// pasted into settings (validated here). `api_version`: `None` falls
    /// back to `DEFAULT_API_VERSION`; pass `Some(..)` whenever the
    /// Preferences UI has a user-supplied value, since that should always
    /// take precedence over this crate's hardcoded default.
    pub fn new(
        secrets: Arc<dyn SecretStore>,
        resource_base_url_raw: &str,
        deployment_name: impl Into<String>,
        api_version: Option<String>,
    ) -> ProviderResult<Self> {
        let client = build_http_client(USER_AGENT)?;
        Self::with_client(
            secrets,
            resource_base_url_raw,
            deployment_name,
            api_version,
            client,
        )
    }

    /// Internal constructor taking the `reqwest::Client` explicitly.
    /// Production code always goes through `new`, which builds an
    /// HTTPS-only client (real Azure resources are always HTTPS); tests
    /// pass a relaxed client here to point at a plain-HTTP `wiremock`
    /// server instead.
    fn with_client(
        secrets: Arc<dyn SecretStore>,
        resource_base_url_raw: &str,
        deployment_name: impl Into<String>,
        api_version: Option<String>,
        client: reqwest::Client,
    ) -> ProviderResult<Self> {
        let mut resource_base_url = validate_endpoint_url(resource_base_url_raw)?;
        if !resource_base_url.path().ends_with('/') {
            let path = format!("{}/", resource_base_url.path());
            resource_base_url.set_path(&path);
        }
        Ok(Self {
            client,
            secrets,
            resource_base_url,
            deployment_name: deployment_name.into(),
            api_version: api_version.unwrap_or_else(|| DEFAULT_API_VERSION.to_string()),
        })
    }

    fn secret_key() -> SecretKey {
        SecretKey::default_for("azure-openai")
    }

    async fn api_key(&self) -> ProviderResult<String> {
        self.secrets
            .get(&Self::secret_key())
            .await
            .map_err(|e| ProviderError::Secret(e.to_string()))?
            .ok_or(ProviderError::NotConfigured)
    }

    fn chat_completions_url(&self) -> ProviderResult<Url> {
        let mut url = self
            .resource_base_url
            .join(&format!(
                "openai/deployments/{}/chat/completions",
                self.deployment_name
            ))
            .map_err(|e| ProviderError::InvalidConfig(format!("invalid Azure OpenAI URL: {e}")))?;
        url.query_pairs_mut()
            .append_pair("api-version", &self.api_version);
        Ok(url)
    }
}

#[async_trait]
impl Provider for AzureOpenAiProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::AzureOpenAi
    }

    fn display_name(&self) -> &'static str {
        "Azure OpenAI"
    }

    fn auth_method(&self) -> AuthMethod {
        AuthMethod::ApiKey
    }

    fn api_key_url(&self) -> Option<&'static str> {
        Some("https://portal.azure.com")
    }

    async fn status(&self) -> ProviderResult<ProviderStatus> {
        let configured = self
            .secrets
            .get(&Self::secret_key())
            .await
            .map_err(|e| ProviderError::Secret(e.to_string()))?
            .is_some();
        if configured {
            Ok(ProviderStatus {
                configured: true,
                reachable: None,
                account_label: None,
                detail: None,
            })
        } else {
            Ok(ProviderStatus::not_configured())
        }
    }

    /// Azure OpenAI does not expose a single "list my deployments" REST
    /// endpoint that is both documented and usable the same way across
    /// every resource, API version, and account tier (the management-plane
    /// deployments API requires Azure Resource Manager credentials, which
    /// are a completely different auth model from the per-resource
    /// `api-key` used for inference calls — inventing a call against it
    /// here would silently fail for most users). Since the deployment
    /// configured on this provider IS the only "model" usable from the
    /// client's perspective, we simply report it as a single entry.
    async fn list_models(&self) -> ProviderResult<Vec<ModelInfo>> {
        Ok(vec![ModelInfo {
            id: self.deployment_name.clone(),
            display_name: self.deployment_name.clone(),
            context_window: None,
            max_output_tokens: None,
            supports_tools: false,
            supports_vision: false,
            input_price_per_million: None,
            output_price_per_million: None,
        }])
    }

    async fn send_chat(&self, request: ChatRequest) -> ProviderResult<ChatStream> {
        let key = self.api_key().await?;
        let url = self.chat_completions_url()?;
        let body = build_request_body(&request);
        let resp = self
            .client
            .post(url)
            .header("api-key", key)
            .json(&body)
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(map_http_error(resp).await);
        }
        Ok(sse_chat_stream(resp))
    }
}

fn build_request_body(request: &ChatRequest) -> serde_json::Value {
    let mut messages = Vec::with_capacity(request.messages.len() + 1);
    if let Some(system) = &request.system_prompt {
        messages.push(serde_json::json!({ "role": "system", "content": system }));
    }
    for m in &request.messages {
        messages.push(serde_json::json!({ "role": role_str(m.role), "content": m.content }));
    }
    // Note: no "model" field — the deployment in the URL path determines
    // which base model actually serves the request.
    let mut body = serde_json::json!({
        "messages": messages,
        "stream": true,
    });
    if let Some(temp) = request.temperature {
        body["temperature"] = serde_json::json!(temp);
    }
    if let Some(max_tokens) = request.max_tokens {
        body["max_tokens"] = serde_json::json!(max_tokens);
    }
    body
}

fn role_str(role: Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
}

async fn map_http_error(resp: reqwest::Response) -> ProviderError {
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    match status.as_u16() {
        401 | 403 => ProviderError::Auth(text),
        404 => ProviderError::ModelNotFound(format!(
            "deployment not found (check the deployment name and api-version): {text}"
        )),
        429 => {
            let retry_after_secs = None; // Azure's Retry-After header is not read here; see note below.
            ProviderError::RateLimited { retry_after_secs }
        }
        _ => ProviderError::InvalidResponse(format!("Azure OpenAI returned HTTP {status}: {text}")),
    }
}

fn map_finish_reason(raw: Option<&str>) -> FinishReason {
    match raw {
        Some("stop") => FinishReason::Stop,
        Some("length") => FinishReason::Length,
        Some("tool_calls") | Some("function_call") => FinishReason::ToolCalls,
        Some("content_filter") => FinishReason::ContentFilter,
        _ => FinishReason::Unknown,
    }
}

#[derive(Debug, Default, Deserialize)]
struct ChunkDelta {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChunkChoice {
    #[serde(default)]
    delta: ChunkDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChunkUsage {
    #[serde(default)]
    prompt_tokens: u32,
    #[serde(default)]
    completion_tokens: u32,
}

#[derive(Debug, Deserialize)]
struct Chunk {
    #[serde(default)]
    choices: Vec<ChunkChoice>,
    #[serde(default)]
    usage: Option<ChunkUsage>,
}

/// Consume a `reqwest::Response` whose body is an OpenAI-style SSE
/// chat-completion stream (`data: {...}` lines, terminated by
/// `data: [DONE]`) and turn it into a `ChatStream`.
fn sse_chat_stream(resp: reqwest::Response) -> ChatStream {
    struct State {
        chunks: std::pin::Pin<
            Box<dyn futures_core::Stream<Item = Result<Vec<u8>, reqwest::Error>> + Send>,
        >,
        buffer: Vec<u8>,
        pending: std::collections::VecDeque<ProviderResult<ChatEvent>>,
        chunks_done: bool,
        done_emitted: bool,
    }

    let state = State {
        chunks: Box::pin(resp.bytes_stream().map(|chunk| chunk.map(|b| b.to_vec()))),
        buffer: Vec::new(),
        pending: std::collections::VecDeque::new(),
        chunks_done: false,
        done_emitted: false,
    };

    Box::pin(futures_util::stream::unfold(
        state,
        |mut state| async move {
            loop {
                if let Some(ev) = state.pending.pop_front() {
                    return Some((ev, state));
                }
                if state.chunks_done {
                    if state.done_emitted {
                        return None;
                    }
                    state.done_emitted = true;
                    return Some((
                        Ok(ChatEvent::Done {
                            finish_reason: FinishReason::Unknown,
                        }),
                        state,
                    ));
                }
                match state.chunks.next().await {
                    Some(Ok(bytes)) => {
                        state.buffer.extend_from_slice(&bytes);
                        while let Some(pos) = state.buffer.iter().position(|&b| b == b'\n') {
                            let mut line: Vec<u8> = state.buffer.drain(..=pos).collect();
                            line.pop(); // drop trailing '\n'
                            if let Some(events) = parse_sse_line(&line) {
                                for ev in events {
                                    if matches!(ev, Ok(ChatEvent::Done { .. })) {
                                        state.done_emitted = true;
                                    }
                                    state.pending.push_back(ev);
                                }
                            }
                        }
                    }
                    Some(Err(e)) => {
                        state.pending.push_back(Err(ProviderError::Network(e)));
                        state.chunks_done = true;
                        state.done_emitted = true;
                    }
                    None => {
                        state.chunks_done = true;
                    }
                }
            }
        },
    ))
}

fn parse_sse_line(line: &[u8]) -> Option<Vec<ProviderResult<ChatEvent>>> {
    let text = std::str::from_utf8(line)
        .unwrap_or("")
        .trim_end_matches('\r');
    let payload = text.strip_prefix("data:")?.trim();
    if payload.is_empty() {
        return None;
    }
    if payload == "[DONE]" {
        return Some(vec![Ok(ChatEvent::Done {
            finish_reason: FinishReason::Stop,
        })]);
    }
    match serde_json::from_str::<Chunk>(payload) {
        Ok(chunk) => {
            let mut events = Vec::new();
            if let Some(usage) = chunk.usage {
                events.push(Ok(ChatEvent::Usage(TokenUsage {
                    input_tokens: usage.prompt_tokens,
                    output_tokens: usage.completion_tokens,
                })));
            }
            for choice in &chunk.choices {
                if let Some(content) = &choice.delta.content
                    && !content.is_empty()
                {
                    events.push(Ok(ChatEvent::TextDelta(content.clone())));
                }
                if let Some(reason) = &choice.finish_reason {
                    events.push(Ok(ChatEvent::Done {
                        finish_reason: map_finish_reason(Some(reason.as_str())),
                    }));
                }
            }
            Some(events)
        }
        Err(e) => Some(vec![Err(ProviderError::InvalidResponse(format!(
            "could not parse Azure OpenAI SSE chunk: {e}"
        )))]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_core::ChatMessage;
    use nexus_core::secrets::SecretError;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    struct FakeSecretStore {
        value: Option<String>,
    }

    #[async_trait]
    impl SecretStore for FakeSecretStore {
        async fn set(&self, _key: &SecretKey, _value: &str) -> Result<(), SecretError> {
            Ok(())
        }
        async fn get(&self, _key: &SecretKey) -> Result<Option<String>, SecretError> {
            Ok(self.value.clone())
        }
        async fn delete(&self, _key: &SecretKey) -> Result<(), SecretError> {
            Ok(())
        }
        async fn list_configured_providers(&self) -> Result<Vec<String>, SecretError> {
            Ok(vec![])
        }
    }

    fn provider_for(uri: &str, api_key: Option<&str>) -> AzureOpenAiProvider {
        let secrets: Arc<dyn SecretStore> = Arc::new(FakeSecretStore {
            value: api_key.map(String::from),
        });
        // `build_local_http_client` (not `build_http_client`) because the
        // `wiremock` server used in these tests is plain HTTP, which the
        // real constructor's HTTPS-only client would refuse. `uri` is a
        // loopback address, so `validate_endpoint_url` accepts the plain
        // `http://` scheme on its own merits.
        let client = nexus_core::http::build_local_http_client(USER_AGENT)
            .expect("test HTTP client should build");
        AzureOpenAiProvider::with_client(secrets, uri, "my-gpt4-deployment", None, client)
            .expect("construction should succeed")
    }

    #[tokio::test]
    async fn list_models_reports_the_configured_deployment() {
        let provider = provider_for("http://127.0.0.1:1", Some("test-key"));
        let models = provider
            .list_models()
            .await
            .expect("list_models should succeed");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "my-gpt4-deployment");
    }

    #[tokio::test]
    async fn sends_api_key_header_and_api_version_query_param() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/openai/deployments/my-gpt4-deployment/chat/completions"))
            .and(query_param("api-version", DEFAULT_API_VERSION))
            .and(header("api-key", "test-key"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                "data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
                "text/event-stream",
            ))
            .mount(&server)
            .await;

        let provider = provider_for(&server.uri(), Some("test-key"));
        let request = ChatRequest {
            model_id: "ignored-for-azure".into(),
            messages: vec![ChatMessage::user("hi")],
            system_prompt: None,
            temperature: None,
            max_tokens: None,
            tools: Vec::new(),
            stream: true,
        };
        let mut stream = provider
            .send_chat(request)
            .await
            .expect("send_chat should succeed");

        let mut text = String::new();
        while let Some(event) = stream.next().await {
            if let ChatEvent::TextDelta(t) = event.expect("no stream errors expected") {
                text.push_str(&t);
            }
        }
        assert_eq!(text, "Hi");
    }

    #[tokio::test]
    async fn missing_api_key_yields_not_configured_not_panic() {
        let provider = provider_for("http://127.0.0.1:1", None);
        let request = ChatRequest {
            model_id: "ignored-for-azure".into(),
            messages: vec![ChatMessage::user("hi")],
            system_prompt: None,
            temperature: None,
            max_tokens: None,
            tools: Vec::new(),
            stream: true,
        };
        let result = provider.send_chat(request).await;
        assert!(matches!(result, Err(ProviderError::NotConfigured)));
    }

    #[tokio::test]
    async fn non_2xx_is_mapped_to_provider_error_not_panic() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(
                "/openai/deployments/my-gpt4-deployment/chat/completions",
            ))
            .respond_with(ResponseTemplate::new(404).set_body_string("deployment not found"))
            .mount(&server)
            .await;

        let provider = provider_for(&server.uri(), Some("test-key"));
        let request = ChatRequest {
            model_id: "ignored-for-azure".into(),
            messages: vec![ChatMessage::user("hi")],
            system_prompt: None,
            temperature: None,
            max_tokens: None,
            tools: Vec::new(),
            stream: true,
        };
        let result = provider.send_chat(request).await;
        assert!(matches!(result, Err(ProviderError::ModelNotFound(_))));
    }
}
