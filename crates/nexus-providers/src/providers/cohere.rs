//! `CohereProvider`: Cohere's hosted Chat API v2 (<https://cohere.com>).
//!
//! VERIFIED: base URL `https://api.cohere.com/v2`, bearer-token auth via
//! `Authorization: Bearer <key>`.
//!
//! UNVERIFIED: the exact shape of the `POST /v2/chat` streaming response
//! and of the `GET /v2/models` listing response below are best-effort,
//! written from general knowledge of Cohere's v2 Chat API rather than from
//! a freshly checked reference document. Cohere's v2 streaming endpoint is
//! known to emit newline-delimited JSON *events* (not classic `data: `
//! Server-Sent Events) where each line is a JSON object carrying a
//! `"type"` discriminator (e.g. `"content-delta"`, `"message-end"`), but
//! the precise field nesting under each event type has not been verified
//! against current, live API documentation. Parsing below is therefore
//! deliberately defensive: unrecognized `"type"` values and missing fields
//! are ignored rather than treated as errors, so a spec drift degrades to
//! "missing some deltas" instead of a hard failure. Anyone revisiting this
//! file should re-verify against Cohere's current docs before relying on
//! it for anything billing-sensitive.

use std::sync::Arc;

use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::Value;
use url::Url;

use nexus_core::http::build_http_client;
use nexus_core::{
    AuthMethod, ChatEvent, ChatRequest, ChatStream, FinishReason, ModelInfo, Provider,
    ProviderError, ProviderKind, ProviderResult, ProviderStatus, Role, SecretKey, SecretStore,
    TokenUsage,
};

const BASE_URL: &str = "https://api.cohere.com/v2/";
const USER_AGENT: &str = concat!("nexus-providers/", env!("CARGO_PKG_VERSION"), " (cohere)");

pub struct CohereProvider {
    client: reqwest::Client,
    secrets: Arc<dyn SecretStore>,
    base_url: Url,
}

impl CohereProvider {
    pub fn new(secrets: Arc<dyn SecretStore>) -> ProviderResult<Self> {
        let client = build_http_client(USER_AGENT)?;
        Ok(Self::with_client(
            secrets,
            Url::parse(BASE_URL).expect("hardcoded base URL is valid"),
            client,
        ))
    }

    /// Internal constructor that takes both the base URL and the
    /// `reqwest::Client` explicitly. The public constructor always uses
    /// the real, fixed base URL and an HTTPS-only client (Cohere has no
    /// per-customer endpoint); tests use this to point at a plain-HTTP
    /// `wiremock` server instead, which `build_http_client`'s
    /// `https_only(true)` would otherwise reject outright.
    fn with_client(secrets: Arc<dyn SecretStore>, base_url: Url, client: reqwest::Client) -> Self {
        Self {
            client,
            secrets,
            base_url,
        }
    }

    fn url(&self, segment: &str) -> ProviderResult<Url> {
        self.base_url
            .join(segment)
            .map_err(|e| ProviderError::InvalidConfig(format!("invalid Cohere URL: {e}")))
    }

    fn secret_key() -> SecretKey {
        SecretKey::default_for("cohere")
    }

    async fn api_key(&self) -> ProviderResult<String> {
        self.secrets
            .get(&Self::secret_key())
            .await
            .map_err(|e| ProviderError::Secret(e.to_string()))?
            .ok_or(ProviderError::NotConfigured)
    }
}

#[async_trait]
impl Provider for CohereProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Cohere
    }

    fn display_name(&self) -> &'static str {
        "Cohere"
    }

    fn auth_method(&self) -> AuthMethod {
        AuthMethod::ApiKey
    }

    fn api_key_url(&self) -> Option<&'static str> {
        Some("https://dashboard.cohere.com/api-keys")
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

    async fn list_models(&self) -> ProviderResult<Vec<ModelInfo>> {
        let key = self.api_key().await?;
        let url = self.url("models")?;
        let resp = self.client.get(url).bearer_auth(key).send().await?;
        if !resp.status().is_success() {
            return Err(map_http_error(resp).await);
        }
        let body: Value = resp
            .json()
            .await
            .map_err(|e| ProviderError::InvalidResponse(format!("bad /v2/models body: {e}")))?;

        // UNVERIFIED shape: tolerate either `{"models": [...]}` or a bare
        // top-level array.
        let entries = body
            .get("models")
            .and_then(Value::as_array)
            .or_else(|| body.as_array())
            .cloned()
            .unwrap_or_default();

        Ok(entries
            .into_iter()
            .filter_map(|m| {
                let id = m
                    .get("name")
                    .or_else(|| m.get("id"))
                    .and_then(Value::as_str)?
                    .to_string();
                let context_window = m
                    .get("context_length")
                    .or_else(|| m.get("context_window"))
                    .and_then(Value::as_u64)
                    .and_then(|n| u32::try_from(n).ok());
                Some(ModelInfo {
                    id: id.clone(),
                    display_name: id,
                    context_window,
                    max_output_tokens: None,
                    supports_tools: false,
                    supports_vision: false,
                    input_price_per_million: None,
                    output_price_per_million: None,
                })
            })
            .collect())
    }

    async fn send_chat(&self, request: ChatRequest) -> ProviderResult<ChatStream> {
        let key = self.api_key().await?;
        let url = self.url("chat")?;
        let body = build_request_body(&request);
        let resp = self
            .client
            .post(url)
            .bearer_auth(key)
            .json(&body)
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(map_http_error(resp).await);
        }
        Ok(ndjson_chat_stream(resp))
    }
}

fn build_request_body(request: &ChatRequest) -> Value {
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
        _ => ProviderError::InvalidResponse(format!("Cohere returned HTTP {status}: {text}")),
    }
}

/// UNVERIFIED: best-effort mapping of Cohere v2's `finish_reason` strings.
/// Unrecognized values map to `FinishReason::Unknown` rather than erroring.
fn map_finish_reason(raw: Option<&str>) -> FinishReason {
    match raw {
        Some("COMPLETE") | Some("complete") => FinishReason::Stop,
        Some("MAX_TOKENS") | Some("max_tokens") => FinishReason::Length,
        Some("TOOL_CALL") | Some("tool_call") | Some("TOOL_CALLS") => FinishReason::ToolCalls,
        Some("ERROR") | Some("error") => FinishReason::Unknown,
        _ => FinishReason::Unknown,
    }
}

/// Consume a `reqwest::Response` whose body is Cohere v2's
/// newline-delimited-JSON chat event stream. See the UNVERIFIED notice at
/// the top of this file: event shapes are parsed defensively.
fn ndjson_chat_stream(resp: reqwest::Response) -> ChatStream {
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
                            if let Some(events) = parse_event_line(&line) {
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
                        if !state.buffer.is_empty() {
                            let line = std::mem::take(&mut state.buffer);
                            if let Some(events) = parse_event_line(&line) {
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
        },
    ))
}

/// Parse one NDJSON line of Cohere's v2 chat stream into zero or more
/// `ChatEvent`s. UNVERIFIED shape — see the module doc comment. Unknown
/// `"type"` values and malformed JSON are both silently ignored (returning
/// `None`) rather than surfaced as errors, since Cohere may add new event
/// types over time and a best-effort stream is more useful than a stream
/// that dies on the first unrecognized line.
fn parse_event_line(line: &[u8]) -> Option<Vec<ProviderResult<ChatEvent>>> {
    let trimmed = std::str::from_utf8(line).unwrap_or("").trim();
    if trimmed.is_empty() {
        return None;
    }
    let value: Value = serde_json::from_str(trimmed).ok()?;
    let event_type = value.get("type").and_then(Value::as_str)?;

    match event_type {
        "content-delta" => {
            let text = value
                .pointer("/delta/message/content/text")
                .and_then(Value::as_str)?;
            if text.is_empty() {
                None
            } else {
                Some(vec![Ok(ChatEvent::TextDelta(text.to_string()))])
            }
        }
        "message-end" => {
            let mut events = Vec::new();
            let input_tokens = value
                .pointer("/delta/usage/billed_units/input_tokens")
                .or_else(|| value.pointer("/delta/usage/tokens/input_tokens"))
                .and_then(Value::as_f64);
            let output_tokens = value
                .pointer("/delta/usage/billed_units/output_tokens")
                .or_else(|| value.pointer("/delta/usage/tokens/output_tokens"))
                .and_then(Value::as_f64);
            if input_tokens.is_some() || output_tokens.is_some() {
                events.push(Ok(ChatEvent::Usage(TokenUsage {
                    input_tokens: input_tokens.unwrap_or(0.0) as u32,
                    output_tokens: output_tokens.unwrap_or(0.0) as u32,
                })));
            }
            let finish_reason = value
                .pointer("/delta/finish_reason")
                .and_then(Value::as_str);
            events.push(Ok(ChatEvent::Done {
                finish_reason: map_finish_reason(finish_reason),
            }));
            Some(events)
        }
        // "message-start", "content-start", "content-end", and any future
        // event type: nothing actionable for us, ignore.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_core::ChatMessage;
    use nexus_core::secrets::SecretError;
    use wiremock::matchers::{method, path};
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

    fn provider_for(uri: &str, api_key: Option<&str>) -> CohereProvider {
        let secrets: Arc<dyn SecretStore> = Arc::new(FakeSecretStore {
            value: api_key.map(String::from),
        });
        let base_url = Url::parse(uri).expect("test server URI is a valid URL");
        // `build_local_http_client` (not `build_http_client`) because the
        // `wiremock` server used in these tests is plain HTTP, which the
        // real constructor's HTTPS-only client would refuse.
        let client = nexus_core::http::build_local_http_client(USER_AGENT)
            .expect("test HTTP client should build");
        CohereProvider::with_client(secrets, base_url, client)
    }

    #[tokio::test]
    async fn parses_list_models() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "models": [{"name": "command-r-plus", "context_length": 128000}]
            })))
            .mount(&server)
            .await;

        let provider = provider_for(&server.uri(), Some("test-key"));
        let models = provider
            .list_models()
            .await
            .expect("list_models should succeed");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "command-r-plus");
        assert_eq!(models[0].context_window, Some(128000));
    }

    #[tokio::test]
    async fn parses_chat_stream_into_events() {
        let server = MockServer::start().await;
        let body = concat!(
            "{\"type\":\"message-start\",\"delta\":{\"message\":{\"role\":\"assistant\"}}}\n",
            "{\"type\":\"content-delta\",\"delta\":{\"message\":{\"content\":{\"text\":\"Hel\"}}}}\n",
            "{\"type\":\"content-delta\",\"delta\":{\"message\":{\"content\":{\"text\":\"lo\"}}}}\n",
            "{\"type\":\"message-end\",\"delta\":{\"finish_reason\":\"COMPLETE\",\"usage\":{\"billed_units\":{\"input_tokens\":4,\"output_tokens\":2}}}}\n",
        );
        Mock::given(method("POST"))
            .and(path("/chat"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/x-ndjson"))
            .mount(&server)
            .await;

        let provider = provider_for(&server.uri(), Some("test-key"));
        let request = ChatRequest {
            model_id: "command-r-plus".into(),
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
        let mut saw_done = false;
        while let Some(event) = stream.next().await {
            match event.expect("no stream errors expected") {
                ChatEvent::TextDelta(t) => text.push_str(&t),
                ChatEvent::Usage(u) => {
                    assert_eq!(u.input_tokens, 4);
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
    async fn missing_api_key_yields_not_configured_not_panic() {
        let server = MockServer::start().await;
        let provider = provider_for(&server.uri(), None);
        let request = ChatRequest {
            model_id: "command-r-plus".into(),
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
            .and(path("/chat"))
            .respond_with(ResponseTemplate::new(401).set_body_string("unauthorized"))
            .mount(&server)
            .await;

        let provider = provider_for(&server.uri(), Some("bad-key"));
        let request = ChatRequest {
            model_id: "command-r-plus".into(),
            messages: vec![ChatMessage::user("hi")],
            system_prompt: None,
            temperature: None,
            max_tokens: None,
            tools: Vec::new(),
            stream: true,
        };
        let result = provider.send_chat(request).await;
        assert!(matches!(result, Err(ProviderError::Auth(_))));
    }
}
