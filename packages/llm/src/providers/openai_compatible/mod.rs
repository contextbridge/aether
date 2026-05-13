#![doc = include_str!(concat!(env!("OUT_DIR"), "/docs/openai_compatible.md"))]

pub mod config;
pub mod generic;
pub mod streaming;
pub mod types;

use async_openai::types::chat::ChatCompletionStreamOptions;
use schemars::Schema;

use crate::providers::openai::mappers::map_tools;
use crate::{Context, LlmError};

pub use config::AetherOpenAiConfig;
pub use streaming::{create_custom_stream_generic, process_compatible_stream};
pub use types::{ChatCompletionStreamResponse, CompatibleChatRequest};

/// Build a chat completion request from a context
///
/// This is shared logic for OpenAI-compatible providers like `OpenRouter` and Z.ai.
/// Uses `CompatibleChatRequest` which preserves `reasoning_content` on assistant messages.
pub fn build_chat_request(
    model: &str,
    context: &Context,
    tool_schema_transform: Option<fn(&mut Schema)>,
) -> Result<CompatibleChatRequest, LlmError> {
    let messages = types::map_messages(context.messages())?;
    let tools =
        if context.tools().is_empty() { None } else { Some(map_tools(context.tools(), tool_schema_transform)?) };

    Ok(CompatibleChatRequest {
        model: model.to_string(),
        messages,
        stream: Some(true),
        tools,
        stream_options: Some(ChatCompletionStreamOptions { include_usage: Some(true), include_obfuscation: None }),
        reasoning_effort: context.reasoning_effort(),
    })
}
