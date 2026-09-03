#![doc = include_str!(concat!(env!("OUT_DIR"), "/docs/openai_compatible.md"))]

pub mod config;
pub mod generic;
pub mod streaming;
pub mod types;

use async_openai::types::chat::ChatCompletionStreamOptions;
use schemars::Schema;

use crate::providers::openai::mappers::map_tools;
use crate::{Context, LlmError};
use types::CompatibleChatRequest;

pub use config::AetherOpenAiConfig;
pub use streaming::{create_custom_stream_generic, process_compatible_stream};
pub use types::ChatCompletionStreamResponse;

/// Selects what `prompt_cache_key` is sent to a LLM provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptCacheKeySource {
    /// Do not send a prompt cache key.
    Omit,
    /// Use a hash of the prompt prefix.
    Prefix,
    /// Use a stable session identifier.
    SessionAffinity,
}

impl PromptCacheKeySource {
    pub(crate) fn resolve(self, context: &Context) -> Option<&str> {
        match self {
            Self::Omit => None,
            Self::Prefix => context.prompt_cache_key(),
            Self::SessionAffinity => context.session_affinity_key(),
        }
    }
}

/// Build a chat completion request from a context
///
/// This is shared logic for OpenAI-compatible providers like `OpenRouter` and Z.ai.
/// Uses `CompatibleChatRequest` which preserves `reasoning_content` on assistant messages.
pub(crate) fn build_chat_request(
    model: &str,
    context: &Context,
    tool_schema_transform: Option<fn(&mut Schema)>,
) -> Result<CompatibleChatRequest, LlmError> {
    let messages = types::map_messages(context.messages())?;
    let tools =
        if context.tools().is_empty() { None } else { Some(map_tools(context.tools(), tool_schema_transform)?) };
    let settings = context.model_settings();

    Ok(CompatibleChatRequest {
        model: model.to_string(),
        messages,
        stream: Some(true),
        tools,
        stream_options: Some(ChatCompletionStreamOptions { include_usage: Some(true), include_obfuscation: None }),
        reasoning_effort: context.reasoning_effort(),
        temperature: settings.temperature,
        top_p: settings.top_p,
        max_tokens: settings.max_tokens,
        prompt_cache_key: None,
    })
}
