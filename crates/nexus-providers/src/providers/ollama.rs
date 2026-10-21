//! `OllamaProvider`: local Ollama server (<https://ollama.com>).
//!
//! VERIFIED against Ollama's documented native API (`/api/tags`, `/api/chat`
//! with `"stream": true`): the server streams newline-delimited JSON
//! objects (NOT Server-Sent Events) over a chunked HTTP response — one
//! object per line, the last one carrying `"done": true` plus token counts.
//!
//! Ollama is a local process the user may or may not have running, so a
//! connection-refused/timeout here is an expected, non-exceptional state
//! (surfaced as `reachable: Some(false)`), not a `ProviderError`.

use async_trait::async_trait;
use futures_util::StreamExt;
use serde::Deserialize;
use url::Url;

use nexus_core::http::{build_local_http_client, validate_endpoint_url};
use nexus_core::{
    AuthMethod, ChatEvent, ChatMessage, ChatRequest, ChatStream, FinishReason, ModelInfo,
    Provider, ProviderError, ProviderKind, ProviderResult, ProviderStatus, Role, TokenUsage,
};

const DEFAULT_BASE_URL: &str = "http://localhost:11434/";
const USER_AGENT: &str = concat!("nexus-providers/", env!("CARGO_PKG_VERSION"), " (ollama)");

pub struct OllamaProvider {
    client: reqwest::Client,
    base_url: Url,
}

impl OllamaProvider {
    /// `custom_base_url`: user-overridable host/port, e.g.
    /// `"http://192.168.1.50:11434"` for Ollama running on another machine
    /// on the user's own LAN. `None` uses the documented default
    /// (`http://localhost:11434`).
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
            .map_err(|e| ProviderError::InvalidConfig(format!("invalid Ollama URL: {e}")))
    }
}

#[async_trait]
impl Provider for OllamaProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Ollama
    }

    fn display_name(&self) -> &'static str {
        "Ollama"
    }

    fn auth_method(&self) -> AuthMethod {
        AuthMethod::None
    }

    fn api_key_url(&self) -> Option<&'static str> {
        None
    }

    async fn status(&self) -> ProviderResult<ProviderStatus> {
        let url = self.url("api/tags")?;
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
                detail: Some(format!("Ollama responded with HTTP {}", resp.status())),
            }),
            Err(e) if e.is_connect() || e.is_timeout() => Ok(ProviderStatus {
                configured: true,
                reachable: Some(false),
                account_label: None,
                detail: Some("Ollama server is not reachable (not running?)".into()),
            }),
            Err(e) => Err(ProviderError::Network(e)),
        }
    }

    async fn list_models(&self) -> ProviderResult<Vec<ModelInfo>> {
        let url = self.url("api/tags")?;
        let resp = self.client.get(url).send().await?;
        if !resp.status().is_success() {
            return Err(map_http_error(resp).await);
        }
        let body: TagsResponse = resp
            .json()
            .await
            .map_err(|e| ProviderError::InvalidResponse(format!("bad /api/tags body: {e}")))?;
        Ok(body
            .models
            .into_iter()
            .map(|m| ModelInfo {
                id: m.name.clone(),
                display_name: m.name,
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
        let url = self.url("api/chat")?;
        let body = build_request_body(&request);
        let resp = self.client.post(url).json(&body).send().await?;
        if !resp.status().is_success() {
            return Err(map_http_error(resp).await);
        }
        Ok(ndjson_chat_stream(resp))
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
    let mut options = serde_json::Map::new();
    if let Some(temp) = request.temperature {
        options.insert("temperature".into(), serde_json::json!(temp));
    }
    if let Some(max_tokens) = request.max_tokens {
        options.insert("num_predict".into(), serde_json::json!(max_tokens));
    }
    let mut body = serde_json::json!({
        "model": request.model_id,
        "messages": messages,
        "stream": true,
    });
    if !options.is_empty() {
        body["options"] = serde_json::Value::Object(options);
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
    if status.as_u16() == 404 {
        ProviderError::ModelNotFound(text)
    } else {
        ProviderError::InvalidResponse(format!("Ollama returned HTTP {status}: {text}"))
    }
}

#[derive(Debug, Deserialize)]
struct TagsResponse {
    #[serde(default)]
    models: Vec<TagModel>,
}

#[derive(Debug, Deserialize)]
struct TagModel {
    name: String,
}

#[derive(Debug, Default, Deserialize)]
struct OllamaStreamLine {
    #[serde(default)]
    message: Option<OllamaMessage>,
    #[serde(default)]
    done: bool,
    #[serde(default)]
    prompt_eval_count: Option<u32>,
    #[serde(default)]
    eval_count: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
struct OllamaMessage {
    #[serde(default)]
    content: String,
}

/// Consume a `reqwest::Response` whose body is Ollama's
/// newline-delimited-JSON chat stream and turn it into a `ChatStream`.
fn ndjson_chat_stream(resp: reqwest::Response) -> ChatStream {
    struct State {
        // Boxed as `Vec<u8>` (rather than the `bytes::Bytes` type `reqwest`
        // returns) so this module does not need `bytes` as a direct
        // dependency just to name the stream's item type.
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

    Box::pin(futures_util::stream::unfold(state, |mut state| async move {
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
                        if let Some(events) = parse_line(&line) {
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
                    state.done_emitted = true; // network error already terminates the stream
                }
                None => {
                    if !state.buffer.is_empty() {
                        let line = std::mem::take(&mut state.buffer);
                        if let Some(events) = parse_line(&line) {
                            for ev in events {
                                if matches!(ev, Ok(ChatEvent::Done { .. })) {
                                    state.done_emitted = true;
                                }
                                state.pending.push_back(ev);
                            }
                        }
                    }
                    state.chunks_done = true;
                }
            }
        }
    }))
}

fn parse_line(line: &[u8]) -> Option<Vec<ProviderResult<ChatEvent>>> {
    let trimmed = std::str::from_utf8(line).unwrap_or("").trim();
    if trimmed.is_empty() {
        return None;
    }
    match serde_json::from_str::<OllamaStreamLine>(trimmed) {
        Ok(line) => {
            let mut events = Vec::new();
            if let Some(message) = &line.message {
                if !message.content.is_empty() {
                    events.push(Ok(ChatEvent::TextDelta(message.content.clone())));
                }
            }
            if line.done {
                events.push(Ok(ChatEvent::Usage(TokenUsage {
                    input_tokens: line.prompt_eval_count.unwrap_or(0),
                    output_tokens: line.eval_count.unwrap_or(0),
                })));
                events.push(Ok(ChatEvent::Done {
                    finish_reason: FinishReason::Stop,
                }));
            }
            Some(events)
        }
        Err(e) => Some(vec![Err(ProviderError::InvalidResponse(format!(
            "could not parse Ollama stream line: {e}"
        )))]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn provider_for(uri: &str) -> OllamaProvider {
        OllamaProvider::new(Some(uri)).expect("valid test URL")
    }

    #[tokio::test]
    async fn parses_list_models() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/tags"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "models": [{"name": "llama3:8b"}, {"name": "phi3:mini"}]
            })))
            .mount(&server)
            .await;

        let provider = provider_for(&server.uri());
        let models = provider.list_models().await.expect("list_models should succeed");
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "llama3:8b");
    }

    #[tokio::test]
    async fn parses_chat_stream_into_events() {
        let server = MockServer::start().await;
        let body = concat!(
            "{\"message\":{\"role\":\"assistant\",\"content\":\"Hel\"},\"done\":false}\n",
            "{\"message\":{\"role\":\"assistant\",\"content\":\"lo\"},\"done\":false}\n",
            "{\"done\":true,\"prompt_eval_count\":5,\"eval_count\":2}\n",
        );
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(body, "application/x-ndjson"),
            )
            .mount(&server)
            .await;

        let provider = provider_for(&server.uri());
        let request = ChatRequest {
            model_id: "llama3:8b".into(),
            messages: vec![ChatMessage::user("hi")],
            system_prompt: None,
            temperature: None,
            max_tokens: None,
            tools: Vec::new(),
            stream: true,
        };
        let mut stream = provider.send_chat(request).await.expect("send_chat should succeed");

        let mut text = String::new();
        let mut saw_usage = false;
        let mut saw_done = false;
        while let Some(event) = stream.next().await {
            match event.expect("no stream errors expected") {
                ChatEvent::TextDelta(t) => text.push_str(&t),
                ChatEvent::Usage(u) => {
                    assert_eq!(u.input_tokens, 5);
                    assert_eq!(u.output_tokens, 2);
                    saw_usage = true;
                }
                ChatEvent::Done { finish_reason } => {
                    assert_eq!(finish_reason, FinishReason::Stop);
                    saw_done = true;
                }
                ChatEvent::ToolCallDelta { .. } => panic!("unexpected tool call"),
            }
        }
        assert_eq!(text, "Hello");
        assert!(saw_usage);
        assert!(saw_done);
    }

    #[tokio::test]
    async fn non_2xx_is_mapped_to_provider_error_not_panic() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
            .mount(&server)
            .await;

        let provider = provider_for(&server.uri());
        let request = ChatRequest {
            model_id: "llama3:8b".into(),
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
        // Port 0 is never a connectable address; this exercises the
        // "local server simply not running" path without flakiness tied to
        // some specific unused port on the test machine.
        let provider = provider_for("http://127.0.0.1:0");
        let status = provider.status().await.expect("status() must not error on connection refused");
        assert!(status.configured);
        assert_eq!(status.reachable, Some(false));
    }
}
