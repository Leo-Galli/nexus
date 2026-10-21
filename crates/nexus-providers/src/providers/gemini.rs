//! Google Gemini provider
//! (`https://generativelanguage.googleapis.com/v1beta`).
//!
//! Confidence notes:
//! - API-key auth via the `?key=` query parameter (not a header), the
//!   `GET /v1beta/models` listing shape (`{"models":[{"name":"models/...",
//!   "displayName", "inputTokenLimit", "outputTokenLimit"}]}`), and the
//!   `POST /v1beta/models/{model}:streamGenerateContent?alt=sse` streaming
//!   endpoint with `candidates[].content.parts[].text` /
//!   `candidates[].finishReason` / `usageMetadata` chunks are **VERIFIED**
//!   against Google's public Gemini API reference as of this writing.
//! - Gemini's chat-turn role names are `"user"` and `"model"` (not
//!   `"assistant"`) — **VERIFIED**; this provider translates
//!   `Role::Assistant` to `"model"`.
//! - **KNOWN LIMITATION**: Gemini's native tool-calling shape
//!   (`functionCall` / `functionResponse` parts) is not modeled here.
//!   `Role::Tool` messages are folded into a plain `user` text part as a
//!   best-effort fallback rather than using the dedicated
//!   `functionResponse` part shape; wire that up when Nexus adds Gemini
//!   tool-calling support. Likewise, no `tools`/`functionDeclarations`
//!   field is sent in the request.
//! - Because the key travels in the URL's query string, request URLs are
//!   never logged or placed in any error message in this file (only
//!   response bodies, redacted, are).

use std::sync::Arc;

use async_trait::async_trait;
use futures_util::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};

use nexus_core::error::{ProviderError, ProviderResult};
use nexus_core::http::build_http_client;
#[cfg(test)]
use nexus_core::http::build_local_http_client;
use nexus_core::model::{ChatEvent, ChatRequest, FinishReason, ModelInfo, Role, TokenUsage};
use nexus_core::provider::{AuthMethod, ChatStream, Provider, ProviderKind, ProviderStatus};
use nexus_core::redact::redact;
use nexus_core::secrets::{SecretKey, SecretStore};

const USER_AGENT: &str = "nexus-providers (gemini)";

pub struct GeminiProvider {
    http: reqwest::Client,
    secrets: Arc<dyn SecretStore>,
    base_url: String,
}

impl GeminiProvider {
    pub const BASE_URL: &'static str = "https://generativelanguage.googleapis.com/v1beta";

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
    fn with_base_url(
        secrets: Arc<dyn SecretStore>,
        base_url: impl Into<String>,
    ) -> ProviderResult<Self> {
        Ok(Self {
            http: build_local_http_client(USER_AGENT)?,
            secrets,
            base_url: base_url.into(),
        })
    }

    fn secret_key() -> SecretKey {
        SecretKey::default_for(ProviderKind::GoogleGemini.as_str())
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
impl Provider for GeminiProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::GoogleGemini
    }

    fn display_name(&self) -> &'static str {
        "Google Gemini"
    }

    fn auth_method(&self) -> AuthMethod {
        AuthMethod::ApiKey
    }

    fn api_key_url(&self) -> Option<&'static str> {
        Some("https://aistudio.google.com/app/apikey")
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
            .query(&[("key", key.as_str())])
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
            .query(&[("key", key.as_str())])
            .send()
            .await
            .map_err(ProviderError::Network)?;

        if !resp.status().is_success() {
            return Err(error_from_response(resp).await);
        }

        let body = resp.text().await.map_err(ProviderError::Network)?;
        let parsed: GeminiModelsResponse = serde_json::from_str(&body).map_err(|e| {
            ProviderError::InvalidResponse(format!("unexpected /models shape: {e}"))
        })?;

        Ok(parsed
            .models
            .into_iter()
            .map(|entry| {
                let id = entry
                    .name
                    .strip_prefix("models/")
                    .unwrap_or(&entry.name)
                    .to_string();
                let display_name = entry.display_name.unwrap_or_else(|| id.clone());
                ModelInfo {
                    id,
                    display_name,
                    context_window: entry.input_token_limit,
                    max_output_tokens: entry.output_token_limit,
                    supports_tools: false,
                    supports_vision: false,
                    input_price_per_million: None,
                    output_price_per_million: None,
                }
            })
            .collect())
    }

    async fn send_chat(&self, request: ChatRequest) -> ProviderResult<ChatStream> {
        let key = self.require_api_key().await?;

        let mut system_parts = Vec::new();
        if let Some(system) = &request.system_prompt
            && !system.is_empty()
        {
            system_parts.push(system.clone());
        }

        let mut contents = Vec::new();
        for m in &request.messages {
            match m.role {
                Role::System => system_parts.push(m.content.clone()),
                Role::User | Role::Tool => contents.push(GeminiContent {
                    role: "user".to_string(),
                    parts: vec![GeminiPart {
                        text: m.content.clone(),
                    }],
                }),
                Role::Assistant => contents.push(GeminiContent {
                    role: "model".to_string(),
                    parts: vec![GeminiPart {
                        text: m.content.clone(),
                    }],
                }),
            }
        }

        let system_instruction = if system_parts.is_empty() {
            None
        } else {
            Some(GeminiSystemInstruction {
                parts: vec![GeminiPart {
                    text: system_parts.join("\n\n"),
                }],
            })
        };

        let body = GeminiRequestBody {
            contents,
            system_instruction,
            generation_config: GeminiGenerationConfig {
                temperature: request.temperature,
                max_output_tokens: request.max_tokens,
            },
        };

        let url = format!(
            "{}/models/{}:streamGenerateContent",
            self.base_url, request.model_id
        );

        let resp = self
            .http
            .post(url)
            .query(&[("alt", "sse"), ("key", key.as_str())])
            .json(&body)
            .send()
            .await
            .map_err(ProviderError::Network)?;

        if !resp.status().is_success() {
            return Err(error_from_response(resp).await);
        }

        Ok(build_gemini_sse_stream(resp))
    }
}

fn build_gemini_sse_stream(resp: reqwest::Response) -> ChatStream {
    let byte_stream = resp.bytes_stream().map(|r| r.map(|b| b.to_vec()));

    let state = GeminiSseState {
        bytes: byte_stream.boxed(),
        buf: String::new(),
        queue: std::collections::VecDeque::new(),
        finished: false,
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
                        if line.trim().is_empty() {
                            continue;
                        }
                        match parse_gemini_chunk(&line) {
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

struct GeminiSseState {
    bytes: futures_core::stream::BoxStream<'static, Result<Vec<u8>, reqwest::Error>>,
    buf: String,
    queue: std::collections::VecDeque<ProviderResult<ChatEvent>>,
    finished: bool,
}

/// Extracts complete SSE `data: ...` payloads, buffering any trailing
/// partial line. Gemini's `alt=sse` stream only uses plain `data:` lines
/// (no named `event:` fields, no `[DONE]` sentinel — the stream simply ends
/// when the HTTP body closes).
fn parse_sse_data_lines(buf: &mut String, chunk: &[u8]) -> Vec<String> {
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

fn parse_gemini_chunk(json: &str) -> ProviderResult<Vec<ChatEvent>> {
    let chunk: GeminiStreamChunk = serde_json::from_str(json)
        .map_err(|e| ProviderError::InvalidResponse(format!("malformed stream chunk: {e}")))?;

    // Emit in content order: any text first, then usage accounting, then
    // the terminal `Done` — Gemini typically bundles the final text part,
    // `finishReason` and `usageMetadata` into one last chunk, and callers
    // reasonably expect text before bookkeeping events.
    let mut events = Vec::new();
    let mut finish_reason = None;

    for candidate in chunk.candidates {
        if let Some(content) = candidate.content {
            for part in content.parts {
                if let Some(text) = part.text
                    && !text.is_empty()
                {
                    events.push(ChatEvent::TextDelta(text));
                }
            }
        }
        if let Some(reason) = candidate.finish_reason {
            finish_reason = Some(map_finish_reason(&reason));
        }
    }

    if let Some(usage) = chunk.usage_metadata {
        events.push(ChatEvent::Usage(TokenUsage {
            input_tokens: usage.prompt_token_count,
            output_tokens: usage.candidates_token_count,
        }));
    }

    if let Some(finish_reason) = finish_reason {
        events.push(ChatEvent::Done { finish_reason });
    }

    Ok(events)
}

fn map_finish_reason(reason: &str) -> FinishReason {
    match reason {
        "STOP" => FinishReason::Stop,
        "MAX_TOKENS" => FinishReason::Length,
        "SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII" => {
            FinishReason::ContentFilter
        }
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
struct GeminiRequestBody {
    contents: Vec<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "systemInstruction")]
    system_instruction: Option<GeminiSystemInstruction>,
    #[serde(rename = "generationConfig")]
    generation_config: GeminiGenerationConfig,
}

#[derive(Debug, Serialize)]
struct GeminiContent {
    role: String,
    parts: Vec<GeminiPart>,
}

#[derive(Debug, Serialize)]
struct GeminiPart {
    text: String,
}

#[derive(Debug, Serialize)]
struct GeminiSystemInstruction {
    parts: Vec<GeminiPart>,
}

#[derive(Debug, Default, Serialize)]
struct GeminiGenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "maxOutputTokens")]
    max_output_tokens: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct GeminiModelsResponse {
    #[serde(default)]
    models: Vec<GeminiModelEntry>,
}

#[derive(Debug, Deserialize)]
struct GeminiModelEntry {
    name: String,
    #[serde(default, rename = "displayName")]
    display_name: Option<String>,
    #[serde(default, rename = "inputTokenLimit")]
    input_token_limit: Option<u32>,
    #[serde(default, rename = "outputTokenLimit")]
    output_token_limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct GeminiStreamChunk {
    #[serde(default)]
    candidates: Vec<GeminiCandidate>,
    #[serde(default, rename = "usageMetadata")]
    usage_metadata: Option<GeminiUsageMetadata>,
}

#[derive(Debug, Deserialize)]
struct GeminiCandidate {
    #[serde(default)]
    content: Option<GeminiContentResp>,
    #[serde(default, rename = "finishReason")]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GeminiContentResp {
    #[serde(default)]
    parts: Vec<GeminiPartResp>,
}

#[derive(Debug, Deserialize)]
struct GeminiPartResp {
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GeminiUsageMetadata {
    #[serde(default, rename = "promptTokenCount")]
    prompt_token_count: u32,
    #[serde(default, rename = "candidatesTokenCount")]
    candidates_token_count: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait as async_trait_test;
    use nexus_core::secrets::SecretError;
    use wiremock::matchers::{method, path, query_param};
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

    fn provider_with_key(base_url: String, key: &str) -> GeminiProvider {
        let secrets = Arc::new(FakeSecretStore {
            key: Some(key.to_string()),
        });
        GeminiProvider::with_base_url(secrets, base_url).unwrap()
    }

    #[tokio::test]
    async fn list_models_parses_fixture() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .and(query_param("key", "test-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "models": [
                    {
                        "name": "models/gemini-1.5-pro",
                        "displayName": "Gemini 1.5 Pro",
                        "inputTokenLimit": 1048576,
                        "outputTokenLimit": 8192
                    }
                ]
            })))
            .mount(&server)
            .await;

        let provider = provider_with_key(server.uri(), "test-key");
        let models = provider.list_models().await.unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "gemini-1.5-pro");
        assert_eq!(models[0].context_window, Some(1048576));
    }

    #[tokio::test]
    async fn send_chat_parses_sse_stream() {
        let server = MockServer::start().await;
        let sse_body = concat!(
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Hel\"}]}}]}\n\n",
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"lo\"}]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":7,\"candidatesTokenCount\":3}}\n\n",
        );

        Mock::given(method("POST"))
            .and(path("/models/gemini-1.5-pro:streamGenerateContent"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_raw(sse_body, "text/event-stream"),
            )
            .mount(&server)
            .await;

        let provider = provider_with_key(server.uri(), "test-key");
        let request = ChatRequest {
            model_id: "gemini-1.5-pro".into(),
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
                input_tokens: 7,
                output_tokens: 3
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
                "error": {"message": "API key not valid"}
            })))
            .mount(&server)
            .await;

        let provider = provider_with_key(server.uri(), "bad-key");
        let err = provider.list_models().await.unwrap_err();
        assert!(matches!(err, ProviderError::Auth(_)));
    }
}
