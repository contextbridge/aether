use schemars::Schema;

use super::types::{
    InputContent, InputItem, MessageContent, Reasoning, ReasoningContent, ResponsesRequest, Role, TextFormat,
    TextParam, Tool,
};
use crate::assistant_reply::replays_encrypted_reasoning;
use crate::providers::openai::wire::{PromptCacheKeySource, reasoning_effort_param};
use crate::tool_schema::normalize_for_xiaomi;
use crate::{
    AssistantContent, ChatMessage, ContentBlock, Context, LlmError, LlmModel, ReasoningEffort, Result, ToolDefinition,
    tool_result_parts,
};

pub struct ResponsesRequestPolicy {
    always_include_reasoning: bool,
    default_effort: Option<ReasoningEffort>,
    disabled_effort: &'static str,
    text_verbosity: Option<&'static str>,
    tool_strict: Option<bool>,
    parallel_tool_calls: Option<bool>,
    tool_schema_transform: Option<fn(&mut Schema)>,
    reasoning_format: ReasoningFormat,
    prompt_cache_key: PromptCacheKeySource,
}

impl ResponsesRequestPolicy {
    pub const OPENAI: Self = Self {
        always_include_reasoning: false,
        default_effort: None,
        disabled_effort: "none",
        text_verbosity: None,
        tool_strict: Some(false),
        parallel_tool_calls: None,
        tool_schema_transform: None,
        reasoning_format: ReasoningFormat::Encrypted,
        prompt_cache_key: PromptCacheKeySource::Prefix,
    };

    pub const XIAOMI: Self = Self {
        tool_schema_transform: Some(normalize_for_xiaomi),
        reasoning_format: ReasoningFormat::PlainText,
        prompt_cache_key: PromptCacheKeySource::Omit,
        ..Self::OPENAI
    };

    #[cfg(feature = "codex")]
    pub const CODEX: Self = Self {
        always_include_reasoning: true,
        default_effort: Some(ReasoningEffort::Medium),
        disabled_effort: "disabled",
        text_verbosity: Some("medium"),
        tool_strict: None,
        parallel_tool_calls: Some(true),
        ..Self::OPENAI
    };

    #[cfg(feature = "bedrock")]
    pub const MANTLE: Self = Self { always_include_reasoning: true, tool_strict: None, ..Self::OPENAI };

    /// Effort to send, if any — an explicit request beats the provider default.
    fn effort(&self, context: &Context) -> Option<ReasoningEffort> {
        match context.reasoning_effort() {
            ReasoningEffort::Default => self.default_effort,
            effort => Some(effort),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReasoningFormat {
    Encrypted,
    PlainText,
}

pub(crate) fn build_request(
    identity: Option<&LlmModel>,
    wire_model: &str,
    context: &Context,
    policy: &ResponsesRequestPolicy,
) -> Result<ResponsesRequest> {
    let encrypted = policy.reasoning_format == ReasoningFormat::Encrypted;
    let (instructions, input) = map_messages(identity, context.messages(), policy.reasoning_format)?;
    let tools = if context.tools().is_empty() { None } else { Some(map_tools(context.tools(), policy)?) };
    let settings = context.model_settings();
    let effort = policy.effort(context);
    let reasoning = (policy.always_include_reasoning || effort.is_some()).then(|| Reasoning {
        summary: (encrypted && context.reasoning_effort() != ReasoningEffort::Disabled).then_some("auto"),
        effort: effort.and_then(|effort| match effort {
            ReasoningEffort::Disabled => Some(policy.disabled_effort),
            effort => reasoning_effort_param(effort),
        }),
    });

    Ok(ResponsesRequest {
        include: encrypted.then_some(&["reasoning.encrypted_content"][..]),
        input,
        instructions,
        max_output_tokens: settings.max_tokens,
        model: wire_model.to_string(),
        parallel_tool_calls: policy.parallel_tool_calls,
        prompt_cache_key: policy.prompt_cache_key.resolve(context).map(String::from),
        reasoning,
        store: false,
        stream: true,
        temperature: settings.temperature,
        text: policy.text_verbosity.map(|verbosity| TextParam { format: TextFormat::Text, verbosity }),
        tools,
        top_p: settings.top_p,
    })
}

fn map_messages(
    identity: Option<&LlmModel>,
    messages: &[ChatMessage],
    reasoning_format: ReasoningFormat,
) -> Result<(Option<String>, Vec<InputItem>)> {
    let mut system_prompt = None;
    let mut items = Vec::new();

    for msg in messages {
        match msg {
            ChatMessage::System { content, .. } => {
                system_prompt = Some(content.clone());
            }
            ChatMessage::User { content, .. } => {
                items.push(InputItem::Message { role: Role::User, content: map_user_content(content)? });
            }
            ChatMessage::Assistant { model, content, .. } => {
                let replay_encrypted = replays_encrypted_reasoning(model.as_ref(), identity);
                items.extend(
                    content
                        .blocks()
                        .iter()
                        .filter_map(|block| map_assistant_block(block, reasoning_format, replay_encrypted)),
                );
            }
            ChatMessage::ToolCallResult(result) => {
                let (call_id, content, is_error) = tool_result_parts(result);
                let output = if is_error { format!("Error: {content}") } else { content.to_string() };
                items.push(InputItem::FunctionCallOutput { call_id: call_id.to_string(), output });
            }
            ChatMessage::Error { .. } | ChatMessage::Summary { .. } => {
                items.extend(msg.as_user_note().map(|note| text_message(Role::User, note)));
            }
        }
    }

    Ok((system_prompt, items))
}

fn map_assistant_block(
    block: &AssistantContent,
    reasoning_format: ReasoningFormat,
    replay_encrypted: bool,
) -> Option<InputItem> {
    match block {
        AssistantContent::Text { text } => (!text.is_empty()).then(|| text_message(Role::Assistant, text.clone())),
        AssistantContent::Reasoning { summary, encrypted } => match reasoning_format {
            ReasoningFormat::PlainText => (!summary.is_empty()).then(|| InputItem::Reasoning {
                id: None,
                summary: vec![],
                encrypted_content: None,
                content: Some(vec![ReasoningContent::ReasoningText { text: summary.clone() }]),
            }),
            ReasoningFormat::Encrypted => {
                encrypted.as_ref().filter(|_| replay_encrypted).map(|encrypted| InputItem::Reasoning {
                    id: Some(encrypted.id.clone()),
                    summary: vec![],
                    encrypted_content: Some(encrypted.content.clone()),
                    content: None,
                })
            }
        },
        AssistantContent::ToolCall(call) => Some(InputItem::FunctionCall {
            arguments: call.arguments.clone(),
            call_id: call.id.clone(),
            name: call.name.clone(),
        }),
    }
}

fn map_user_content(parts: &[ContentBlock]) -> Result<MessageContent> {
    let mut items = Vec::with_capacity(parts.len());
    for part in parts {
        match part {
            ContentBlock::Text { text } => items.push(InputContent::InputText { text: text.clone() }),
            ContentBlock::Image { .. } => items.push(InputContent::InputImage {
                detail: "auto",
                image_url: part.as_data_uri().expect("image content always has a data URI"),
            }),
            ContentBlock::Audio { .. } => {
                return Err(LlmError::UnsupportedContent("OpenAI Responses does not support audio input".into()));
            }
        }
    }
    Ok(MessageContent::Parts(items))
}

fn map_tools(tools: &[ToolDefinition], policy: &ResponsesRequestPolicy) -> Result<Vec<Tool>> {
    tools
        .iter()
        .map(|tool| {
            Ok(Tool::Function {
                name: tool.name.clone(),
                parameters: tool.parameters_with(policy.tool_schema_transform)?,
                strict: policy.tool_strict,
                description: tool.description.clone(),
            })
        })
        .collect()
}

fn text_message(role: Role, content: String) -> InputItem {
    InputItem::Message { role, content: MessageContent::Text(content) }
}
