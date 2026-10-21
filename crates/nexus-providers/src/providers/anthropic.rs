//! Anthropic (Claude) provider (`https://api.anthropic.com/v1`).
//!
//! Confidence notes:
//! - Auth via `x-api-key` header (not `Authorization: Bearer`) plus the
//!   required `anthropic-version` header, the `/v1/messages` streaming
//!   event shape (`message_start`, `content_block_delta` with
//!   `text_delta`, `message_delta` carrying incremental `usage`, and
//!   `message_stop`), and the fact that `system` is a top-level request
//!   field rather than a message role, are all **VERIFIED** against
//!   Anthropic's public Messages API reference as of this writing.
//! - `GET /v1/models` returning `{"data": [{"id", "display_name", ...}]}`
//!   is **VERIFIED** against Anthropic's models-list endpoint docs.
//! - **KNOWN LIMITATION**: this implementation does not send a `tools`
//!   field in requests, and does not parse `content_block_start` /
//!   `input_json_delta` tool-use events into `ChatEvent::ToolCallDelta`.
//!   Anthropic tool-calling was out of scope for this pass; `ChatRequest`
//!   messages carrying `Role::Tool` are still mapped to a `tool_result`
//!   content block on a `user` message so a conversation that already
//!   contains tool results (e.g. replayed from another provider) degrades
//!   gracefully rather than erroring, but Nexus cannot yet *originate* a
//!   tool call through this provider. Add `tools` request mapping and
//!   `tool_use` event parsing here when Anthropic tool-calling is wired up.
//! - `message_delta.delta.stop_reason` values (`end_turn`,
//!   `max_tokens`, `stop_sequence`, `tool_use`) are mapped to
//!   `FinishReason` on a best-effort basis; `message_stop` itself carries
//!   no payload, so the mapped reason from the preceding `message_delta` is
//!   cached and used when `message_stop` arrives (defaulting to `Stop` if
//!   no `message_delta` was seen, which should not happen in practice).

use std::sync::Arc;

use async_trait::async_trait;
use futures_util::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;

use nexus_core::error::{ProviderError, ProviderResult};
use nexus_core::http::build_http_client;
#[cfg(test)]
use nexus_core::http::build_local_http_client;
use nexus_core::model::{ChatEvent, ChatRequest, FinishReason, ModelInfo, Role, TokenUsage};
use nexus_core::provider::{AuthMethod, ChatStream, Provider, ProviderKind, ProviderStatus};
use nexus_core::redact::redact;
use nexus_core::secrets::{SecretKey, SecretStore};

const USER_AGENT: &str = "nexus-providers (anthropic)";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MAX_TOKENS: u32 = 4096;

pub struct AnthropicProvider {
    http: reqwest::Client,
    secrets: Arc<dyn SecretStore>,
    base_url: String,
}

impl AnthropicProvider {
    pub const BASE_URL: &'static str = "https://api.anthropic.com/v1";

    pub fn new(secrets: Arc<dyn SecretStore>) -> ProviderResult<Self> {
        Ok(Self {
            http: build_http_client(USER_AGENT)?,
            secrets,
            base_url: Self::BASE_URL.to_string(),
        })
    }

    /// Test-only: points at a local `wiremock` server; see the identical
    /// note in `openai.rs`.
    #[cfg(test)]
    fn with_base_url(secrets: Arc<dyn SecretStore>, base_url: impl Into<String>) -> ProviderResult<Self> {
        Ok(Self {
            http: build_local_http_client(USER_AGENT)?,
            secrets,
            base_url: base_url.into(),
        })
    }

    fn secret_key() -> SecretKey {
        SecretKey::default_for(ProviderKind::Anthropic.as_str())
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
impl Provider for AnthropicProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Anthropic
    }

    fn display_name(&self) -> &'static str {
        "Anthropic"
    }

    fn auth_method(&self) -> AuthMethod {
        AuthMethod::ApiKey
    }

    fn api_key_url(&self) -> Option<&'static str> {
        Some("https://console.anthropic.com/settings/keys")
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

        match self
            .http
            .get(format!("{}/models", self.base_url))
            .header("x-api-key", &key)
            .header("anthropic-version", ANTHROPIC_VERSION)
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
            .header("x-api-key", &key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .send()
            .await
            .map_err(ProviderError::Network)?;

        if !resp.status().is_success() {
            return Err(error_from_response(resp).await);
        }

        let body = resp.text().await.map_err(ProviderError::Network)?;
        let parsed: AnthropicModelsResponse = serde_json::from_str(&body)
            .map_err(|e| ProviderError::InvalidResponse(format!("unexpected /models shape: {e}")))?;

        Ok(parsed
            .data
            .into_iter()
            .map(|entry| ModelInfo {
                id: entry.id,
                display_name: entry.display_name,
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

        let mut system_parts = Vec::new();
        if let Some(system) = &request.system_prompt
            && !system.is_empty() {
                system_parts.push(system.clone());
            }

        let mut messages = Vec::new();
        for m in &request.messages {
            match m.role {
                Role::System => system_parts.push(m.content.clone()),
                Role::User => messages.push(AnthropicMessage {
                    role: "user".to_string(),
                    content: json!(m.content),
                }),
                Role::Assistant => messages.push(AnthropicMessage {
                    role: "assistant".to_string(),
                    content: json!(m.content),
                }),
                Role::Tool => messages.push(AnthropicMessage {
                    role: "user".to_string(),
                    content: json!([{
                        "type": "tool_result",
                        "tool_use_id": m.tool_call_id.clone().unwrap_or_default(),
                        "content": m.content,
                    }]),
                }),
            }
        }

        let system = if system_parts.is_empty() {
            None
        } else {
            Some(system_parts.join("\n\n"))
        };

        let body = AnthropicRequestBody {
            model: &request.model_id,
            max_tokens: request.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
            system,
            messages,
            stream: true,
            temperature: request.temperature,
        };

        let resp = self
            .http
            .post(format!("{}/messages", self.base_url))
            .header("x-api-key", &key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&body)
            .send()
            .await
            .map_err(ProviderError::Network)?;

        if !resp.status().is_success() {
            return Err(error_from_response(resp).await);
        }

        Ok(build_anthropic_sse_stream(resp))
    }
}

fn build_anthropic_sse_stream(resp: reqwest::Response) -> ChatStream {
    let byte_stream = resp.bytes_stream().map(|r| r.map(|b| b.to_vec()));

    let state = AnthropicSseState {
        bytes: byte_stream.boxed(),
        buf: String::new(),
        current_event: None,
        current_data: String::new(),
        queue: std::collections::VecDeque::new(),
        finished: false,
        pending_input_tokens: None,
        pending_finish_reason: None,
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
                    let lines = drain_lines(&mut state.buf, &chunk);
                    for line in lines {
                        if line.is_empty() {
                            // Blank line: dispatch the accumulated event.
                            if !state.current_data.is_empty() {
                                let event_name =
                                    state.current_event.take().unwrap_or_default();
                                let data = std::mem::take(&mut state.current_data);
                                match parse_anthropic_event(
                                    &event_name,
                                    &data,
                                    &mut state.pending_input_tokens,
                                    &mut state.pending_finish_reason,
                                ) {
                                    Ok(events) => {
                                        state.queue.extend(events.into_iter().map(Ok));
                                    }
                                    Err(e) => {
                                        state.queue.push_back(Err(e));
                                        state.finished = true;
                                    }
                                }
                            }
                            state.current_event = None;
                            continue;
                        }
                        if let Some(name) = line.strip_prefix("event:") {
                            state.current_event = Some(name.trim().to_string());
                        } else if let Some(data) = line.strip_prefix("data:") {
                            if !state.current_data.is_empty() {
                                state.current_data.push('\n');
                            }
                            state.current_data.push_str(data.trim_start());
                        }
                        // Other SSE fields (id:, comments) are ignored.
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

struct AnthropicSseState {
    bytes: futures_core::stream::BoxStream<'static, Result<Vec<u8>, reqwest::Error>>,
    buf: String,
    current_event: Option<String>,
    current_data: String,
    queue: std::collections::VecDeque<ProviderResult<ChatEvent>>,
    finished: bool,
    pending_input_tokens: Option<u32>,
    pending_finish_reason: Option<FinishReason>,
}

/// Returns complete raw lines (without the trailing `\n`/`\r\n`) found in
/// `chunk`, buffering a trailing partial line in `buf`.
fn drain_lines(buf: &mut String, chunk: &[u8]) -> Vec<String> {
    buf.push_str(&String::from_utf8_lossy(chunk));
    let mut lines = Vec::new();
    while let Some(pos) = buf.find('\n') {
        let line: String = buf[..pos].to_string();
        buf.drain(..=pos);
        lines.push(line.trim_end_matches('\r').to_string());
    }
    lines
}

fn parse_anthropic_event(
    event: &str,
    data: &str,
    pending_input_tokens: &mut Option<u32>,
    pending_finish_reason: &mut Option<FinishReason>,
) -> ProviderResult<Vec<ChatEvent>> {
    match event {
        "message_start" => {
            let parsed: AnthropicMessageStart = serde_json::from_str(data).map_err(|e| {
                ProviderError::InvalidResponse(format!("malformed message_start: {e}"))
            })?;
            *pending_input_tokens = Some(parsed.message.usage.input_tokens);
            Ok(Vec::new())
        }
        "content_block_delta" => {
            let parsed: AnthropicContentBlockDelta = serde_json::from_str(data).map_err(|e| {
                ProviderError::InvalidResponse(format!("malformed content_block_delta: {e}"))
            })?;
            if parsed.delta.kind == "text_delta"
                && let Some(text) = parsed.delta.text {
                    return Ok(vec![ChatEvent::TextDelta(text)]);
                }
            Ok(Vec::new())
        }
        "message_delta" => {
            let parsed: AnthropicMessageDelta = serde_json::from_str(data).map_err(|e| {
                ProviderError::InvalidResponse(format!("malformed message_delta: {e}"))
            })?;
            if let Some(stop_reason) = parsed.delta.stop_reason {
                *pending_finish_reason = Some(map_stop_reason(&stop_reason));
            }
            let mut events = Vec::new();
            if let Some(usage) = parsed.usage {
                events.push(ChatEvent::Usage(TokenUsage {
                    input_tokens: pending_input_tokens.take().unwrap_or(0),
                    output_tokens: usage.output_tokens,
                }));
            }
            Ok(events)
        }
        "message_stop" => Ok(vec![ChatEvent::Done {
            finish_reason: pending_finish_reason.take().unwrap_or(FinishReason::Stop),
        }]),
        "error" => Err(ProviderError::InvalidResponse(format!(
            "stream error event: {}",
            redact(data)
        ))),
        _ => Ok(Vec::new()),
    }
}

fn map_stop_reason(reason: &str) -> FinishReason {
    match reason {
        "end_turn" | "stop_sequence" => FinishReason::Stop,
        "max_tokens" => FinishReason::Length,
        "tool_use" => FinishReason::ToolCalls,
        _ => FinishReason::Unknown,
    }
}

async fn error_from_response(resp: reqwest::Response) -> ProviderError {
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

#[derive(Debug, Serialize)]
struct AnthropicRequestBody<'a> {
    model: &'a str,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    messages: Vec<AnthropicMessage>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[derive(Debug, Serialize)]
struct AnthropicMessage {
    role: String,
    content: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct AnthropicModelsResponse {
    #[serde(default)]
    data: Vec<AnthropicModelEntry>,
}

#[derive(Debug, Deserialize)]
struct AnthropicModelEntry {
    id: String,
    display_name: String,
}

#[derive(Debug, Deserialize)]
struct AnthropicMessageStart {
    message: AnthropicMessageStartInner,
}

#[derive(Debug, Deserialize)]
struct AnthropicMessageStartInner {
    usage: AnthropicInputUsage,
}

#[derive(Debug, Deserialize)]
struct AnthropicInputUsage {
    input_tokens: u32,
}

#[derive(Debug, Deserialize)]
struct AnthropicContentBlockDelta {
    delta: AnthropicTextDelta,
}

#[derive(Debug, Deserialize)]
struct AnthropicTextDelta {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicMessageDelta {
    #[serde(default)]
    delta: AnthropicMessageDeltaInner,
    #[serde(default)]
    usage: Option<AnthropicOutputUsage>,
}

#[derive(Debug, Default, Deserialize)]
struct AnthropicMessageDeltaInner {
    #[serde(default)]
    stop_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicOutputUsage {
    output_tokens: u32,
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

    fn provider_with_key(base_url: String, key: &str) -> AnthropicProvider {
        let secrets = Arc::new(FakeSecretStore {
            key: Some(key.to_string()),
        });
        AnthropicProvider::with_base_url(secrets, base_url).unwrap()
    }

    #[tokio::test]
    async fn list_models_parses_fixture() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .and(header("x-api-key", "test-key"))
            .and(header("anthropic-version", ANTHROPIC_VERSION))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [
                    {"id": "claude-3-5-sonnet-20241022", "display_name": "Claude 3.5 Sonnet"}
                ]
            })))
            .mount(&server)
            .await;

        let provider = provider_with_key(server.uri(), "test-key");
        let models = provider.list_models().await.unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "claude-3-5-sonnet-20241022");
        assert_eq!(models[0].display_name, "Claude 3.5 Sonnet");
    }

    #[tokio::test]
    async fn send_chat_parses_sse_stream() {
        let server = MockServer::start().await;
        let sse_body = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":25}}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hi\"}}\n\n",
            "event: message_delta\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":5}}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );

        Mock::given(method("POST"))
            .and(path("/messages"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_raw(sse_body, "text/event-stream"),
            )
            .mount(&server)
            .await;

        let provider = provider_with_key(server.uri(), "test-key");
        let request = ChatRequest {
            model_id: "claude-3-5-sonnet-20241022".into(),
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
                input_tokens: 25,
                output_tokens: 5
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
                "error": {"type": "authentication_error", "message": "invalid x-api-key"}
            })))
            .mount(&server)
            .await;

        let provider = provider_with_key(server.uri(), "bad-key");
        let err = provider.list_models().await.unwrap_err();
        assert!(matches!(err, ProviderError::Auth(_)));
    }
}
