use crate::{ReasoningEffort, TokenUsage};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum CacheType {
    Ephemeral,
}

/// Messages API request body
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Request {
    pub messages: Vec<Message>,
    pub max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<Vec<ContentBlock>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Tool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<Thinking>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_config: Option<OutputConfig>,
}

#[derive(Debug, Serialize)]
pub(crate) struct StreamingRequest {
    model: String,
    stream: bool,
    #[serde(flatten)]
    request: Request,
}

impl StreamingRequest {
    pub(crate) fn new(model: impl Into<String>, request: Request) -> Self {
        Self { model: model.into(), stream: true, request }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub(crate) enum Thinking {
    Enabled { budget_tokens: u32 },
    Adaptive { display: ThinkingDisplay },
    Disabled,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ThinkingDisplay {
    Summarized,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct OutputConfig {
    pub effort: ReasoningEffort,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub(crate) enum ContentBlock {
    #[serde(rename = "text")]
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    #[serde(rename = "image")]
    Image {
        source: ImageSource,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String, input: serde_json::Value },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    #[serde(rename = "thinking")]
    Thinking { thinking: String, signature: String },
    #[serde(rename = "redacted_thinking")]
    RedactedThinking { data: String },
}

impl ContentBlock {
    pub(crate) fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into(), cache_control: None }
    }

    pub(crate) fn is_thinking(&self) -> bool {
        matches!(self, Self::Thinking { .. } | Self::RedactedThinking { .. })
    }

    pub(crate) fn is_tool_result(&self) -> bool {
        matches!(self, Self::ToolResult { .. })
    }

    pub(crate) fn mark_cache_breakpoint(&mut self) {
        match self {
            Self::Text { cache_control, .. }
            | Self::Image { cache_control, .. }
            | Self::ToolResult { cache_control, .. } => {
                *cache_control = Some(CacheControl::ephemeral());
            }
            Self::ToolUse { .. } | Self::Thinking { .. } | Self::RedactedThinking { .. } => {}
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ImageSource {
    #[serde(rename = "type")]
    pub source_type: String,
    pub media_type: String,
    pub data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CacheControl {
    #[serde(rename = "type")]
    pub cache_type: CacheType,
}

impl CacheControl {
    pub(crate) fn ephemeral() -> Self {
        Self { cache_type: CacheType::Ephemeral }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct Tool {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum StreamEvent {
    MessageStart(MessageStart),
    ContentBlockStart(ContentBlockStart),
    ContentBlockDelta(ContentBlockDelta),
    ContentBlockStop(ContentBlockStop),
    MessageDelta(MessageDelta),
    MessageStop,
    Error(ErrorEvent),
    Ping,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct MessageStart {
    pub message: MessageStartData,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct MessageStartData {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(clippy::struct_field_names)]
pub struct Usage {
    pub input_tokens: Option<u32>,
    pub output_tokens: Option<u32>,
    pub cache_creation_input_tokens: Option<u32>,
    pub cache_read_input_tokens: Option<u32>,
}

impl Usage {
    pub(crate) fn merge(&self, later: &Usage) -> Usage {
        Usage {
            input_tokens: later.input_tokens.or(self.input_tokens),
            output_tokens: later.output_tokens.or(self.output_tokens),
            cache_creation_input_tokens: later.cache_creation_input_tokens.or(self.cache_creation_input_tokens),
            cache_read_input_tokens: later.cache_read_input_tokens.or(self.cache_read_input_tokens),
        }
    }
}

impl From<&Usage> for TokenUsage {
    fn from(usage: &Usage) -> Self {
        TokenUsage::from_uncached_input(
            usage.input_tokens.unwrap_or(0),
            usage.output_tokens.unwrap_or(0),
            usage.cache_read_input_tokens,
            usage.cache_creation_input_tokens,
        )
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct ContentBlockStart {
    pub index: u32,
    pub content_block: ContentBlockStartData,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub(crate) enum ContentBlockStartData {
    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String },
    #[serde(rename = "thinking")]
    Thinking,
    #[serde(rename = "redacted_thinking")]
    RedactedThinking { data: String },
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct ContentBlockDelta {
    pub index: u32,
    pub delta: ContentBlockDeltaData,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(clippy::enum_variant_names)]
pub(crate) enum ContentBlockDeltaData {
    TextDelta { text: String },
    InputJsonDelta { partial_json: String },
    ThinkingDelta { thinking: String },
    SignatureDelta { signature: String },
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ContentBlockStop {
    pub index: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct MessageDelta {
    pub delta: MessageDeltaData,
    #[serde(default)]
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct MessageDeltaData {
    pub stop_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct ErrorEvent {
    pub error: Error,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct Error {
    #[serde(rename = "type")]
    pub error_type: String,
    pub message: String,
}
