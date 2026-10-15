pub(crate) mod streaming;
pub(crate) mod types;

use async_openai::types::chat::{ChatCompletionStreamOptions, ChatCompletionTool, ChatCompletionTools, FunctionObject};
use schemars::Schema;
use utils::ReasoningEffort;

use crate::{Context, LlmError, ToolDefinition};
use types::CompatibleChatRequest;

pub use streaming::process_compatible_stream;

#[derive(Debug, Clone, Copy)]
pub struct ChatRequestPolicy {
    pub(crate) tool_schema_transform: Option<fn(&mut Schema)>,
    pub(crate) prompt_cache_key: PromptCacheKeySource,
    pub(crate) tool_call_reasoning_placeholder: bool,
    pub(crate) customize: Option<fn(&mut CompatibleChatRequest, &Context)>,
}

impl ChatRequestPolicy {
    pub const DEFAULT: Self = Self {
        tool_schema_transform: None,
        prompt_cache_key: PromptCacheKeySource::Omit,
        tool_call_reasoning_placeholder: true,
        customize: None,
    };
}

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

pub(crate) fn reasoning_effort_param(effort: ReasoningEffort) -> Option<&'static str> {
    match effort {
        ReasoningEffort::Default => None,
        ReasoningEffort::Disabled => Some("none"),
        effort => Some(effort.as_str()),
    }
}

/// Build a chat completion request from a context
///
/// This is shared logic for OpenAI-compatible providers like `OpenRouter` and Z.ai.
/// Uses `CompatibleChatRequest` which preserves `reasoning_content` on assistant messages.
pub(crate) fn build_chat_request(
    model: &str,
    context: &Context,
    policy: &ChatRequestPolicy,
) -> Result<CompatibleChatRequest, LlmError> {
    let messages = types::map_messages(context.messages(), policy.tool_call_reasoning_placeholder)?;
    let tools =
        if context.tools().is_empty() { None } else { Some(map_tools(context.tools(), policy.tool_schema_transform)?) };
    let settings = context.model_settings();

    let mut request = CompatibleChatRequest {
        model: model.to_string(),
        messages,
        stream: Some(true),
        tools,
        stream_options: Some(ChatCompletionStreamOptions { include_usage: Some(true), include_obfuscation: None }),
        reasoning_effort: reasoning_effort_param(context.reasoning_effort()),
        temperature: settings.temperature,
        top_p: settings.top_p,
        max_tokens: settings.max_tokens,
        prompt_cache_key: policy.prompt_cache_key.resolve(context).map(String::from),
        extra: serde_json::Map::new(),
    };
    if let Some(customize) = policy.customize {
        customize(&mut request, context);
    }
    Ok(request)
}

fn map_tools(
    tools: &[ToolDefinition],
    tool_schema_transform: Option<fn(&mut Schema)>,
) -> Result<Vec<ChatCompletionTools>, LlmError> {
    tools
        .iter()
        .map(|tool| {
            Ok(ChatCompletionTools::Function(ChatCompletionTool {
                function: FunctionObject {
                    name: tool.name.clone(),
                    description: Some(tool.description.clone()),
                    parameters: Some(tool.parameters_with(tool_schema_transform)?),
                    strict: Some(false),
                },
            }))
        })
        .collect()
}
