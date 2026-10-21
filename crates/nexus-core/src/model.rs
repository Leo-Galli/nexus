use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// Raw JSON arguments, as returned by the provider (may be streamed in
    /// fragments and only valid JSON once complete).
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
    /// Set on `Role::Tool` messages: which tool call this is a result for.
    pub tool_call_id: Option<String>,
    /// Set on `Role::Assistant` messages that requested tool use.
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
}

impl ChatMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            tool_call_id: None,
            tool_calls: Vec::new(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            tool_call_id: None,
            tool_calls: Vec::new(),
        }
    }
}

/// A tool (function) definition offered to the model, using the JSON
/// Schema subset every mainstream provider accepts for parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters_schema: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    pub model_id: String,
    pub messages: Vec<ChatMessage>,
    pub system_prompt: Option<String>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub tools: Vec<ToolDefinition>,
    /// Always `true` in practice: Nexus always streams. Kept explicit so
    /// provider implementations don't silently assume it.
    pub stream: bool,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    Length,
    ToolCalls,
    ContentFilter,
    Unknown,
}

/// One unit of a streamed chat response. Provider implementations yield a
/// `futures_core::Stream<Item = ProviderResult<ChatEvent>>`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ChatEvent {
    TextDelta(String),
    ToolCallDelta {
        id: String,
        name: Option<String>,
        arguments_delta: String,
    },
    Usage(TokenUsage),
    Done {
        finish_reason: FinishReason,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub display_name: String,
    pub context_window: Option<u32>,
    pub max_output_tokens: Option<u32>,
    pub supports_tools: bool,
    pub supports_vision: bool,
    /// USD per 1M input tokens, from the hand-maintained pricing table in
    /// `nexus-stats`. `None` when the price is not known — never guessed.
    pub input_price_per_million: Option<f64>,
    pub output_price_per_million: Option<f64>,
}
