use crate::{TokenUsage, Tokens};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheType {
    Ephemeral,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Request {
    pub model: String,
    pub messages: Vec<Message>,
    pub max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<SystemContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Tool>>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<Thinking>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Thinking {
    #[serde(rename = "type")]
    pub thinking_type: String,
    pub budget_tokens: u32,
}

impl Thinking {
    pub fn new(budget_tokens: u32) -> Self {
        Self { thinking_type: "enabled".to_string(), budget_tokens }
    }
}

impl Request {
    pub fn new(model: String, messages: Vec<Message>) -> Self {
        Self {
            model,
            messages,
            max_tokens: 4096,
            temperature: None,
            top_p: None,
            system: None,
            tools: None,
            stream: false,
            thinking: None,
            cache_control: None,
        }
    }

    pub fn with_auto_caching(mut self) -> Self {
        self.cache_control = Some(CacheControl::ephemeral());
        self
    }

    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }

    pub fn with_top_p(mut self, top_p: f32) -> Self {
        self.top_p = Some(top_p);
        self
    }

    pub fn with_system_cached(mut self, system: String) -> Self {
        self.system = Some(SystemContent::Blocks(vec![SystemContentBlock::Text {
            text: system,
            cache_control: Some(CacheControl::ephemeral()),
        }]));
        self
    }

    pub fn with_tools(mut self, tools: Vec<Tool>) -> Self {
        self.tools = Some(tools);
        self
    }

    pub fn with_stream(mut self, stream: bool) -> Self {
        self.stream = stream;
        self
    }

    pub fn with_thinking(mut self, thinking: Thinking) -> Self {
        self.thinking = Some(thinking);
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Message {
    pub role: Role,
    pub content: Content,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Content {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SystemContent {
    Text(String),
    Blocks(Vec<SystemContentBlock>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SystemContentBlock {
    #[serde(rename = "text")]
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    #[serde(rename = "image")]
    Image { source: ImageSource },
    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String, input: serde_json::Value },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageSource {
    #[serde(rename = "type")]
    pub source_type: String,
    pub media_type: String,
    pub data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheControl {
    #[serde(rename = "type")]
    pub cache_type: CacheType,
}

impl CacheControl {
    pub fn ephemeral() -> Self {
        Self { cache_type: CacheType::Ephemeral }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    MessageStart {
        #[serde(flatten)]
        data: MessageStart,
    },
    ContentBlockStart {
        #[serde(flatten)]
        data: ContentBlockStart,
    },
    ContentBlockDelta {
        #[serde(flatten)]
        data: ContentBlockDelta,
    },
    ContentBlockStop {
        #[serde(flatten)]
        data: ContentBlockStop,
    },
    MessageDelta {
        #[serde(flatten)]
        data: MessageDelta,
    },
    MessageStop {
        #[serde(flatten)]
        data: MessageStop,
    },
    #[serde(rename = "error")]
    Error {
        #[serde(flatten)]
        data: ErrorEvent,
    },
    #[serde(rename = "ping")]
    Ping,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MessageStart {
    pub message: ResponseMessage,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ResponseMessage {
    pub id: String,
    #[serde(rename = "type")]
    pub message_type: String,
    pub role: Role,
    pub content: Vec<serde_json::Value>,
    pub model: String,
    pub stop_reason: Option<String>,
    pub stop_sequence: Option<String>,
    pub usage: Usage,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u32>,
}

impl From<&Usage> for TokenUsage {
    fn from(usage: &Usage) -> Self {
        let cache_read = usage.cache_read_input_tokens.map(Tokens::from);
        let cache_creation = usage.cache_creation_input_tokens.map(Tokens::from);
        // Anthropic's input_tokens excludes cached tokens; TokenUsage counts the whole prompt.
        let cached = cache_read.unwrap_or_default() + cache_creation.unwrap_or_default();
        TokenUsage {
            input_tokens: Tokens::from(usage.input_tokens) + cached,
            output_tokens: usage.output_tokens.into(),
            cache_read_tokens: cache_read,
            cache_creation_tokens: cache_creation,
            ..TokenUsage::default()
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ContentBlockStart {
    pub index: u32,
    pub content_block: ContentBlockStartData,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlockStartData {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String },
    #[serde(rename = "thinking")]
    Thinking { thinking: String },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ContentBlockDelta {
    pub index: u32,
    pub delta: ContentBlockDeltaData,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlockDeltaData {
    TextDelta { text: String },
    InputJsonDelta { partial_json: String },
    ThinkingDelta { thinking: String },
}

#[derive(Debug, Clone, Deserialize)]
pub struct ContentBlockStop {
    pub index: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct MessageDelta {
    pub delta: MessageDeltaData,
    #[serde(default)]
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct MessageDeltaData {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_sequence: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MessageStop {}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ErrorEvent {
    pub error: Error,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Error {
    #[serde(rename = "type")]
    pub error_type: String,
    pub message: String,
}
