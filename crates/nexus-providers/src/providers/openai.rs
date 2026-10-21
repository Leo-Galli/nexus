//! OpenAI provider (`https://api.openai.com/v1`).
//!
//! Confidence notes:
//! - `GET /v1/models` and `POST /v1/chat/completions` (with `"stream": true`)
//!   request/response shapes below are **VERIFIED** against OpenAI's public
//!   API reference as of this writing.
//! - The per-model fields OpenAI's `/v1/models` endpoint returns (`id`,
//!   `object`, `created`, `owned_by`) do **not** reliably include a context
//!   window, max-output-tokens, modality support, or pricing. Rather than
//!   guess those, `list_models` leaves `context_window`,
//!   `max_output_tokens`, `input_price_per_million` and
//!   `output_price_per_million` as `None` and `supports_tools` /
//!   `supports_vision` as a best-effort `false`. Per-model capability and
//!   pricing enrichment belongs in `nexus-stats`'s hand-maintained
//!   metadata/pricing table, not guessed in this crate.
//! - Streaming usage accounting requires asking for it explicitly via
//!   `"stream_options": {"include_usage": true}` (**VERIFIED**: documented
//!   OpenAI behavior) — this provider always sets that flag so a `Usage`
//!   event is available before the stream ends.
//! - The base URL is intentionally hard-coded and not user-configurable;
//!   use `OpenAiCompatibleProvider` for OpenAI-API-compatible services that
//!   need a different base URL.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use async_trait::async_trait;
use futures_util::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};

use nexus_core::error::{ProviderError, ProviderResult};
use nexus_core::http::build_http_client;
#[cfg(test)]
use nexus_core::http::build_local_http_client;
use nexus_core::model::{
    ChatEvent, ChatRequest, FinishReason, ModelInfo, Role, TokenUsage, ToolDefinition,
};
use nexus_core::provider::{AuthMethod, ChatStream, Provider, ProviderKind, ProviderStatus};
use nexus_core::redact::redact;
use nexus_core::secrets::{SecretKey, SecretStore};

const USER_AGENT: &str = "nexus-providers (openai)";

pub struct OpenAiProvider {
    http: reqwest::Client,
    secrets: Arc<dyn SecretStore>,
    /// Hard-coded in production; overridable only for tests so we can point
    /// at a local `wiremock` server instead of `https://api.openai.com/v1`.
    base_url: String,
}

impl OpenAiProvider {
    pub const BASE_URL: &'static str = "https://api.openai.com/v1";

    pub fn new(secrets: Arc<dyn SecretStore>) -> ProviderResult<Self> {
        Ok(Self {
            http: build_http_client(USER_AGENT)?,
            secrets,
            base_url: Self::BASE_URL.to_string(),
        })
    }

    /// Test-only: points at a local `wiremock` server instead of
    /// `BASE_URL`, and uses a client without `https_only` (the production
    /// constructor keeps that restriction since the real OpenAI API is
    /// always `https`).
    #[cfg(test)]
    fn with_base_url(secrets: Arc<dyn SecretStore>, base_url: impl Into<String>) -> ProviderResult<Self> {
        Ok(Self {
            http: build_local_http_client(USER_AGENT)?,
            secrets,
            base_url: base_url.into(),
        })
    }

    fn secret_key() -> SecretKey {
        SecretKey::default_for(ProviderKind::OpenAi.as_str())
    }

    async fn require_api_key(&self) -> ProviderResult<String> {
        self.secrets
            .get(&Self::secret_key())
            .await
            .map_err(|e| ProviderError::Secret(e.to_string()))?
            .ok_or(ProviderError::NotConfigured)
    }
}

#[async_trait]
impl Provider for OpenAiProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::OpenAi
    }

    fn display_name(&self) -> &'static str {
        "OpenAI"
    }

    fn auth_method(&self) -> AuthMethod {
        AuthMethod::ApiKey
    }

    fn api_key_url(&self) -> Option<&'static str> {
        Some("https://platform.openai.com/api-keys")
    }

    async fn status(&self) -> ProviderResult<ProviderStatus> {
        let key = self
            .secrets
            .get(&Self::secret_key())
            .await
            .map_err(|e| ProviderError::Secret(e.to_string()))?;
        let Some(key) = key else {
            return Ok(ProviderStatus::not_configured());
        };

        // Listing models is not a billed request, so it doubles as a cheap
        // reachability probe.
        match self
            .http
            .get(format!("{}/models", self.base_url))
            .bearer_auth(&key)
            .send()
            .await
        {
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
        let key = self.require_api_key().await?;
        let resp = self
            .http
            .get(format!("{}/models", self.base_url))
            .bearer_auth(&key)
            .send()
            .await
            .map_err(ProviderError::Network)?;

        if !resp.status().is_success() {
            return Err(error_from_response(resp).await);
        }

        let body = resp.text().await.map_err(ProviderError::Network)?;
        let parsed: OpenAiModelsResponse = serde_json::from_str(&body)
            .map_err(|e| ProviderError::InvalidResponse(format!("unexpected /models shape: {e}")))?;

        Ok(parsed
            .data
            .into_iter()
            .map(|entry| ModelInfo {
                id: entry.id.clone(),
                display_name: entry.id,
                // Not reliably exposed by this endpoint; see module doc.
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
        let key = self.require_api_key().await?;

        let mut messages = Vec::new();
        if let Some(system) = &request.system_prompt {
            if !system.is_empty() {
                messages.push(OpenAiMessage {
                    role: "system".to_string(),
                    content: system.clone(),
                    tool_call_id: None,
                });
            }
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
            stream_options: OpenAiStreamOptions { include_usage: true },
            temperature: request.temperature,
            max_tokens: request.max_tokens,
            tools,
        };

        let resp = self
            .http
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(&key)
            .json(&body)
            .send()
            .await
            .map_err(ProviderError::Network)?;

        if !resp.status().is_success() {
            return Err(error_from_response(resp).await);
        }

        Ok(build_openai_sse_stream(resp))
    }
}

/// Builds the `ChatStream` from a successful, still-streaming OpenAI (or
/// OpenAI-compatible) chat completions response. Shared by `openai.rs` and
/// `openai_compatible.rs` by duplication (see crate-level note on why no
/// shared module is used).
pub(crate) fn build_openai_sse_stream(resp: reqwest::Response) -> ChatStream {
    let byte_stream = resp.bytes_stream().map(|r| r.map(|b| b.to_vec()));

    let state = OpenAiSseState {
        bytes: byte_stream.boxed(),
        buf: String::new(),
        queue: VecDeque::new(),
        finished: false,
        tool_call_ids: HashMap::new(),
    };

    stream::unfold(state, |mut state| async move {
        loop {
            if let Some(event) = state.queue.pop_front() {
                return Some((event, state));
            }
            if state.finished {
                return None;
            }
            match state.bytes.next().await {
                Some(Ok(chunk)) => {
                    let lines = parse_sse_data_lines(&mut state.buf, &chunk);
                    for line in lines {
                        if line.trim() == "[DONE]" {
                            state.finished = true;
                            continue;
                        }
                        if line.trim().is_empty() {
                            continue;
                        }
                        match parse_openai_chunk(&line, &mut state.tool_call_ids) {
                            Ok(events) => state.queue.extend(events.into_iter().map(Ok)),
                            Err(e) => {
                                state.queue.push_back(Err(e));
                                state.finished = true;
                            }
                        }
                    }
                }
                Some(Err(e)) => {
                    state.finished = true;
                    return Some((Err(ProviderError::Network(e)), state));
                }
                None => {
                    state.finished = true;
                }
            }
        }
    })
    .boxed()
}

struct OpenAiSseState {
    bytes: futures_core::stream::BoxStream<'static, Result<Vec<u8>, reqwest::Error>>,
    buf: String,
    queue: VecDeque<ProviderResult<ChatEvent>>,
    finished: bool,
    /// OpenAI only sends a tool call's `id` on the first delta for that
    /// call; later deltas for the same `index` omit it. We remember it here
    /// so every `ChatEvent::ToolCallDelta` we emit carries the real id.
    tool_call_ids: HashMap<u32, String>,
}

/// Extracts complete SSE `data: ...` payloads out of a raw byte chunk,
/// buffering any trailing partial line in `buf` for the next call. Lines
/// that are not `data:` fields (blank separator lines, comments) are
/// dropped — fine for OpenAI/Gemini-style streams that never send other
/// named SSE fields we care about.
pub(crate) fn parse_sse_data_lines(buf: &mut String, chunk: &[u8]) -> Vec<String> {
    buf.push_str(&String::from_utf8_lossy(chunk));
    let mut out = Vec::new();
    while let Some(pos) = buf.find('\n') {
        let line: String = buf[..pos].to_string();
        buf.drain(..=pos);
        let line = line.trim_end_matches('\r');
        if let Some(data) = line.strip_prefix("data:") {
            out.push(data.trim_start().to_string());
        }
    }
    out
}

/// Maps one decoded SSE `data:` JSON payload from an OpenAI-shaped chat
/// completions stream into zero or more `ChatEvent`s.
pub(crate) fn parse_openai_chunk(
    json: &str,
    tool_call_ids: &mut HashMap<u32, String>,
) -> ProviderResult<Vec<ChatEvent>> {
    let chunk: OpenAiStreamChunk = serde_json::from_str(json)
        .map_err(|e| ProviderError::InvalidResponse(format!("malformed stream chunk: {e}")))?;

    // Emit in content order: text/tool-call deltas first, then usage
    // accounting, then the terminal `Done` — kept consistent with the same
    // policy in `gemini.rs`. In practice OpenAI sends `finish_reason` and
    // `usage` in separate chunks (the latter only when
    // `stream_options.include_usage` is set, with an empty `choices`
    // array), so this ordering rarely matters for real traffic, but it
    // keeps behavior well-defined when a chunk happens to carry more than
    // one of these at once.
    let mut events = Vec::new();
    let mut finish_reason = None;

    for choice in chunk.choices {
        if let Some(content) = choice.delta.content {
            if !content.is_empty() {
                events.push(ChatEvent::TextDelta(content));
            }
        }
        for tc in choice.delta.tool_calls {
            let index = tc.index.unwrap_or(0);
            if let Some(id) = &tc.id {
                tool_call_ids.insert(index, id.clone());
            }
            let id = tool_call_ids.get(&index).cloned().unwrap_or_default();
            let (name, arguments_delta) = match tc.function {
                Some(f) => (f.name, f.arguments.unwrap_or_default()),
                None => (None, String::new()),
            };
            events.push(ChatEvent::ToolCallDelta {
                id,
                name,
                arguments_delta,
            });
        }
        if let Some(reason) = choice.finish_reason {
            finish_reason = Some(map_finish_reason(&reason));
        }
    }

    if let Some(usage) = chunk.usage {
        events.push(ChatEvent::Usage(TokenUsage {
            input_tokens: usage.prompt_tokens,
            output_tokens: usage.completion_tokens,
        }));
    }

    if let Some(finish_reason) = finish_reason {
        events.push(ChatEvent::Done { finish_reason });
    }

    Ok(events)
}

pub(crate) fn map_finish_reason(reason: &str) -> FinishReason {
    match reason {
        "stop" => FinishReason::Stop,
        "length" => FinishReason::Length,
        "tool_calls" => FinishReason::ToolCalls,
        "content_filter" => FinishReason::ContentFilter,
        _ => FinishReason::Unknown,
    }
}

pub(crate) fn role_to_str(role: Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
}

/// Maps a non-2xx response into a typed `ProviderError`. The response body
/// is passed through `redact()` as defence-in-depth, but note we never read
/// or echo back the request's `Authorization` header in the first place.
pub(crate) async fn error_from_response(resp: reqwest::Response) -> ProviderError {
    let status = resp.status();
    let retry_after = resp
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());
    let body = resp.text().await.unwrap_or_default();
    let safe_body = redact(&body);

    match status.as_u16() {
        401 | 403 => ProviderError::Auth(safe_body),
        404 => ProviderError::ModelNotFound(safe_body),
        429 => ProviderError::RateLimited {
            retry_after_secs: retry_after,
        },
        _ => ProviderError::InvalidResponse(format!("HTTP {status}: {safe_body}")),
    }
}

/// Shared with `openai_compatible.rs`, since "OpenAI-compatible" means
/// exactly this request shape.
#[derive(Debug, Serialize)]
pub(crate) struct OpenAiChatRequestBody<'a> {
    pub(crate) model: &'a str,
    pub(crate) messages: Vec<OpenAiMessage>,
    pub(crate) stream: bool,
    pub(crate) stream_options: OpenAiStreamOptions,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) tools: Vec<OpenAiToolDef>,
}

#[derive(Debug, Serialize)]
pub(crate) struct OpenAiStreamOptions {
    pub(crate) include_usage: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct OpenAiMessage {
    pub(crate) role: String,
    pub(crate) content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tool_call_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct OpenAiToolDef {
    #[serde(rename = "type")]
    kind: &'static str,
    function: OpenAiFunctionDef,
}

impl OpenAiToolDef {
    pub(crate) fn from_definition(def: &ToolDefinition) -> Self {
        Self {
            kind: "function",
            function: OpenAiFunctionDef {
                name: def.name.clone(),
                description: def.description.clone(),
                parameters: def.parameters_schema.clone(),
            },
        }
    }
}

#[derive(Debug, Serialize)]
struct OpenAiFunctionDef {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct OpenAiModelsResponse {
    #[serde(default)]
    data: Vec<OpenAiModelEntry>,
}

#[derive(Debug, Deserialize)]
struct OpenAiModelEntry {
    id: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OpenAiStreamChunk {
    #[serde(default)]
    choices: Vec<OpenAiStreamChoice>,
    #[serde(default)]
    usage: Option<OpenAiUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAiStreamChoice {
    #[serde(default)]
    delta: OpenAiDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct OpenAiDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<OpenAiToolCallDelta>,
}

#[derive(Debug, Deserialize)]
struct OpenAiToolCallDelta {
    #[serde(default)]
    index: Option<u32>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<OpenAiFunctionDelta>,
}

#[derive(Debug, Default, Deserialize)]
struct OpenAiFunctionDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiUsage {
    #[serde(default)]
    prompt_tokens: u32,
    #[serde(default)]
    completion_tokens: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait as async_trait_test;
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

    fn provider_with_key(base_url: String, key: &str) -> OpenAiProvider {
        let secrets = Arc::new(FakeSecretStore {
            key: Some(key.to_string()),
        });
        OpenAiProvider::with_base_url(secrets, base_url).unwrap()
    }

    #[tokio::test]
    async fn list_models_parses_fixture() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .and(header("authorization", "Bearer test-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [
                    {"id": "gpt-4o", "object": "model", "owned_by": "openai"},
                    {"id": "gpt-4o-mini", "object": "model", "owned_by": "openai"}
                ]
            })))
            .mount(&server)
            .await;

        let provider = provider_with_key(server.uri(), "test-key");
        let models = provider.list_models().await.unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "gpt-4o");
        assert!(models[0].context_window.is_none());
    }

    #[tokio::test]
    async fn send_chat_parses_sse_stream() {
        let server = MockServer::start().await;
        let sse_body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":2}}\n\n",
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

        let provider = provider_with_key(server.uri(), "test-key");
        let request = ChatRequest {
            model_id: "gpt-4o".into(),
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

        assert_eq!(events.len(), 4);
        assert!(matches!(&events[0], ChatEvent::TextDelta(s) if s == "Hel"));
        assert!(matches!(&events[1], ChatEvent::TextDelta(s) if s == "lo"));
        assert!(matches!(
            &events[2],
            ChatEvent::Usage(TokenUsage {
                input_tokens: 10,
                output_tokens: 2
            })
        ));
        assert!(matches!(
            &events[3],
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
                "error": {"message": "invalid api key"}
            })))
            .mount(&server)
            .await;

        let provider = provider_with_key(server.uri(), "bad-key");
        let err = provider.list_models().await.unwrap_err();
        assert!(matches!(err, ProviderError::Auth(_)));
    }
}
