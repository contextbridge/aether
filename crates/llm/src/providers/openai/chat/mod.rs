pub(crate) mod streaming;
pub(crate) mod types;
use super::wire::{PromptCacheKeySource, reasoning_effort_param};
use crate::{Context, LlmError, ToolDefinition};
use schemars::Schema;
pub use streaming::process_compatible_stream;
use types::{CompatibleChatRequest, FunctionDefinition, StreamOptions, Tool};

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
        stream: true,
        tools,
        stream_options: StreamOptions { include_usage: true },
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

fn map_tools(tools: &[ToolDefinition], tool_schema_transform: Option<fn(&mut Schema)>) -> Result<Vec<Tool>, LlmError> {
    tools
        .iter()
        .map(|tool| {
            Ok(Tool::Function {
                function: FunctionDefinition {
                    name: tool.name.clone(),
                    description: tool.description.clone(),
                    parameters: tool.parameters_with(tool_schema_transform)?,
                    strict: false,
                },
            })
        })
        .collect()
}
