use serde::Serialize;

#[derive(Debug, Serialize)]
pub(crate) struct ResponsesRequest {
    pub input: Vec<InputItem>,
    pub stream: bool,
    #[serde(flatten)]
    pub settings: RequestSettings,
}

#[derive(Debug, PartialEq, Serialize)]
pub(crate) struct RequestSettings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include: Option<&'static [&'static str]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallel_tool_calls: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_cache_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<Reasoning>,
    pub store: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<TextParam>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Tool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
}

#[derive(Debug, PartialEq, Serialize)]
pub(crate) struct Reasoning {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<&'static str>,
}

#[derive(Debug, PartialEq, Serialize)]
pub(crate) struct TextParam {
    pub format: TextFormat,
    pub verbosity: &'static str,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum TextFormat {
    Text,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum InputItem {
    Message {
        role: Role,
        content: MessageContent,
    },
    Reasoning {
        id: Option<String>,
        summary: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        encrypted_content: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<Vec<ReasoningContent>>,
    },
    FunctionCall {
        arguments: String,
        call_id: String,
        name: String,
    },
    FunctionCallOutput {
        call_id: String,
        output: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Role {
    Developer,
    User,
    Assistant,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub(crate) enum MessageContent {
    Text(String),
    Parts(Vec<InputContent>),
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum InputContent {
    InputText { text: String },
    InputImage { detail: &'static str, image_url: String },
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ReasoningContent {
    ReasoningText { text: String },
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum Tool {
    Function {
        name: String,
        parameters: serde_json::Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        strict: Option<bool>,
        description: String,
    },
}
