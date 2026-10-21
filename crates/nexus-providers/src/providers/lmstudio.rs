//! `LmStudioProvider`: local LM Studio server (<https://lmstudio.ai>).
//!
//! VERIFIED: LM Studio's local inference server exposes an
//! OpenAI-compatible HTTP API (`GET /v1/models`, `POST
//! /v1/chat/completions` with `"stream": true` returning classic
//! Server-Sent Events, terminated by a `data: [DONE]` line) — this is
//! LM Studio's documented, stable local-server surface.
//!
//! Like Ollama, LM Studio is a local process that may simply not be
//! running; connection failures are therefore surfaced as
//! `reachable: Some(false)`, never as a hard error from `status()`.

use async_trait::async_trait;
use futures_util::StreamExt;
use serde::Deserialize;
use url::Url;

use nexus_core::http::{build_local_http_client, validate_endpoint_url};
use nexus_core::{
    AuthMethod, ChatEvent, ChatRequest, ChatStream, FinishReason, ModelInfo, Provider,
    ProviderError, ProviderKind, ProviderResult, ProviderStatus, Role, TokenUsage,
};

const DEFAULT_BASE_URL: &str = "http://localhost:1234/";
const USER_AGENT: &str = concat!(
    "nexus-providers/",
    env!("CARGO_PKG_VERSION"),
    " (lm-studio)"
);

pub struct LmStudioProvider {
    client: reqwest::Client,
    base_url: Url,
}

impl LmStudioProvider {
    /// `custom_base_url`: user-overridable host/port. `None` uses LM
    /// Studio's documented default (`http://localhost:1234`).
    pub fn new(custom_base_url: Option<&str>) -> ProviderResult<Self> {
        let base_url = match custom_base_url {
            Some(raw) => validate_endpoint_url(raw)?,
            None => Url::parse(DEFAULT_BASE_URL).expect("hardcoded default URL is valid"),
        };
        let client = build_local_http_client(USER_AGENT)?;
        Ok(Self { client, base_url })
    }

    fn url(&self, segment: &str) -> ProviderResult<Url> {
        self.base_url
            .join(segment)
            .map_err(|e| ProviderError::InvalidConfig(format!("invalid LM Studio URL: {e}")))
    }
}

#[async_trait]
impl Provider for LmStudioProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::LmStudio
    }

    fn display_name(&self) -> &'static str {
        "LM Studio"
    }

    fn auth_method(&self) -> AuthMethod {
        AuthMethod::None
    }

    fn api_key_url(&self) -> Option<&'static str> {
        None
    }

    async fn status(&self) -> ProviderResult<ProviderStatus> {
        let url = self.url("v1/models")?;
        match self.client.get(url).send().await {
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
                detail: Some(format!("LM Studio responded with HTTP {}", resp.status())),
            }),
            Err(e) if e.is_connect() || e.is_timeout() => Ok(ProviderStatus {
                configured: true,
                reachable: Some(false),
                account_label: None,
                detail: Some("LM Studio server is not reachable (not running?)".into()),
            }),
            Err(e) => Err(ProviderError::Network(e)),
        }
    }

    async fn list_models(&self) -> ProviderResult<Vec<ModelInfo>> {
        let url = self.url("v1/models")?;
        let resp = self.client.get(url).send().await?;
        if !resp.status().is_success() {
            return Err(map_http_error(resp).await);
        }
        let body: ModelsResponse = resp
            .json()
            .await
            .map_err(|e| ProviderError::InvalidResponse(format!("bad /v1/models body: {e}")))?;
        Ok(body
            .data
            .into_iter()
            .map(|m| ModelInfo {
                id: m.id.clone(),
                display_name: m.id,
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
        let url = self.url("v1/chat/completions")?;
        let body = build_request_body(&request);
        let resp = self.client.post(url).json(&body).send().await?;
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
    let mut body = serde_json::json!({
        "model": request.model_id,
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
        404 => ProviderError::ModelNotFound(text),
        429 => ProviderError::RateLimited {
            retry_after_secs: None,
        },
        _ => ProviderError::InvalidResponse(format!("LM Studio returned HTTP {status}: {text}")),
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

#[derive(Debug, Deserialize)]
struct ModelsResponse {
    #[serde(default)]
    data: Vec<ModelEntry>,
}

#[derive(Debug, Deserialize)]
struct ModelEntry {
    id: String,
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

/// Consume a `reqwest::Response` whose body is an OpenAI-compatible SSE
/// chat-completion stream (`data: {...}` lines, terminated by
/// `data: [DONE]`) and turn it into a `ChatStream`.
///
/// This is a small, self-contained SSE reader duplicated across the
/// local-server providers (LM Studio, llama.cpp) on purpose: each provider
/// module must compile and be testable standalone.
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
            "could not parse LM Studio SSE chunk: {e}"
        )))]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_core::ChatMessage;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn provider_for(uri: &str) -> LmStudioProvider {
        LmStudioProvider::new(Some(uri)).expect("valid test URL")
    }

    #[tokio::test]
    async fn parses_list_models() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "object": "list",
                "data": [{"id": "mlx-community/llama-3-8b"}]
            })))
            .mount(&server)
            .await;

        let provider = provider_for(&server.uri());
        let models = provider
            .list_models()
            .await
            .expect("list_models should succeed");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "mlx-community/llama-3-8b");
    }

    #[tokio::test]
    async fn parses_sse_chat_stream_into_events() {
        let server = MockServer::start().await;
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2}}\n\n",
            "data: [DONE]\n\n",
        );
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/event-stream"))
            .mount(&server)
            .await;

        let provider = provider_for(&server.uri());
        let request = ChatRequest {
            model_id: "local-model".into(),
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
        let mut saw_usage = false;
        let mut done_count = 0;
        while let Some(event) = stream.next().await {
            match event.expect("no stream errors expected") {
                ChatEvent::TextDelta(t) => text.push_str(&t),
                ChatEvent::Usage(u) => {
                    assert_eq!(u.input_tokens, 3);
                    assert_eq!(u.output_tokens, 2);
                    saw_usage = true;
                }
                ChatEvent::Done { .. } => done_count += 1,
                ChatEvent::ToolCallDelta { .. } => panic!("unexpected tool call"),
            }
        }
        assert_eq!(text, "Hello");
        assert!(saw_usage);
        assert!(done_count >= 1);
    }

    #[tokio::test]
    async fn non_2xx_is_mapped_to_provider_error_not_panic() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
            .mount(&server)
            .await;

        let provider = provider_for(&server.uri());
        let request = ChatRequest {
            model_id: "local-model".into(),
            messages: vec![ChatMessage::user("hi")],
            system_prompt: None,
            temperature: None,
            max_tokens: None,
            tools: Vec::new(),
            stream: true,
        };
        let result = provider.send_chat(request).await;
        assert!(matches!(result, Err(ProviderError::InvalidResponse(_))));
    }

    #[tokio::test]
    async fn status_reports_unreachable_on_connection_refused_without_erroring() {
        let provider = provider_for("http://127.0.0.1:0");
        let status = provider
            .status()
            .await
            .expect("status() must not error on connection refused");
        assert!(status.configured);
        assert_eq!(status.reachable, Some(false));
    }
}
