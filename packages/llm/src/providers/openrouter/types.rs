use async_openai::types::chat::{
    ChatCompletionStreamOptions, ChatCompletionToolChoiceOption, ChatCompletionTools, ResponseFormat, StopConfiguration,
};
use serde::{Deserialize, Serialize};

use crate::providers::openai_compatible::CompatibleChatRequest;
use crate::providers::openai_compatible::types::CompatibleChatMessage;

/// OpenRouter-specific usage configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenRouterUsage {
    #[serde(rename = "include")]
    pub include: bool,
}

/// Cache control marker for `OpenRouter` prompt caching.
/// Enables automatic prefix caching and sticky routing.
/// See: <https://openrouter.ai/docs/guides/best-practices/prompt-caching>
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheControl {
    #[serde(rename = "type")]
    pub cache_type: String,
}

impl CacheControl {
    pub fn ephemeral() -> Self {
        Self { cache_type: "ephemeral".to_string() }
    }
}

/// Custom request type for `OpenRouter` that includes the usage parameter
///
/// `OpenRouter` requires a specific `usage` parameter in the request body to enable
/// token usage tracking. See: <https://openrouter.ai/docs/use-cases/usage-accounting>
#[derive(Debug, Clone, Serialize)]
pub struct OpenRouterChatRequest {
    pub model: String,
    pub messages: Vec<CompatibleChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ChatCompletionTools>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ChatCompletionToolChoiceOption>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_completion_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<ChatCompletionStreamOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<OpenRouterUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<StopConfiguration>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<ResponseFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<crate::ReasoningEffort>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}

impl From<CompatibleChatRequest> for OpenRouterChatRequest {
    fn from(request: CompatibleChatRequest) -> Self {
        Self {
            model: request.model,
            messages: request.messages,
            stream: request.stream,
            tools: request.tools,
            tool_choice: None,
            temperature: request.temperature,
            top_p: request.top_p,
            max_completion_tokens: request.max_tokens,
            stream_options: Some(ChatCompletionStreamOptions { include_usage: Some(true), include_obfuscation: None }),
            usage: Some(OpenRouterUsage { include: true }),
            presence_penalty: None,
            frequency_penalty: None,
            stop: None,
            response_format: None,
            reasoning_effort: None,
            cache_control: Some(CacheControl::ephemeral()),
        }
    }
}
