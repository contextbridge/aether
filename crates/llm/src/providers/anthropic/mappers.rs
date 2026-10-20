use super::types::{
    ContentBlock as AnthropicContentBlock, ImageSource, Message, OutputConfig, Request, Role, Thinking,
    ThinkingDisplay, Tool,
};
use crate::reasoning::ReasoningControl;
use crate::{
    AssistantReasoning, ChatMessage, ContentBlock, Context, LlmError, LlmModel, ReasoningEffort, Result,
    ToolCallRequest, ToolDefinition, tool_result_parts,
};
use tracing::debug;

/// Tokens requested when no `max_tokens` is set via `ModelSettings`. Anthropic requires
/// `max_tokens` on every request, so a value is always sent.
const DEFAULT_MAX_TOKENS: u32 = 16_384;

pub(crate) fn build_request(identity: Option<&LlmModel>, context: &Context) -> Result<Request> {
    let context = context.filter_encrypted_reasoning(identity);
    let (mut system, mut messages) = map_messages(context.messages())?;
    if let Some(block) = system.last_mut() {
        block.mark_cache_breakpoint();
    }
    if let Some(block) = messages.last_mut().and_then(|message| message.content.last_mut()) {
        block.mark_cache_breakpoint();
    }
    let settings = context.model_settings();

    let mut request = Request {
        messages,
        max_tokens: settings.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
        temperature: settings.temperature,
        top_p: settings.top_p,
        system: (!system.is_empty()).then_some(system),
        tools: (!context.tools().is_empty()).then(|| map_tools(context.tools())),
        thinking: None,
        output_config: None,
    };
    let control = identity.map(LlmModel::reasoning_control).unwrap_or_default();
    apply_reasoning(&mut request, control, context.reasoning_effort());
    Ok(request)
}

fn map_messages(messages: &[ChatMessage]) -> Result<(Vec<AnthropicContentBlock>, Vec<Message>)> {
    let mut system = Vec::new();
    let mut mapped = Vec::new();

    for message in messages {
        match message {
            ChatMessage::System { content, .. } => system.push(AnthropicContentBlock::text(content.as_str())),
            ChatMessage::User { content, .. } => {
                mapped.push(Message { role: Role::User, content: map_content_blocks(content)? });
            }
            ChatMessage::Assistant { content, tool_calls, reasoning, .. } => {
                mapped.push(map_assistant_message(content, tool_calls, reasoning));
            }
            ChatMessage::ToolCallResult(result) => {
                let (tool_use_id, content, is_error) = tool_result_parts(result);
                push_tool_result(
                    &mut mapped,
                    AnthropicContentBlock::ToolResult {
                        tool_use_id: tool_use_id.to_string(),
                        content: content.to_string(),
                        is_error: Some(is_error),
                        cache_control: None,
                    },
                );
            }
            ChatMessage::Error { .. } | ChatMessage::Summary { .. } => {
                let note = message.as_user_note();
                mapped.extend(
                    note.map(|note| Message { role: Role::User, content: vec![AnthropicContentBlock::text(note)] }),
                );
            }
        }
    }

    Ok((system, mapped))
}

fn map_tools(tools: &[ToolDefinition]) -> Vec<Tool> {
    tools
        .iter()
        .map(|tool| Tool {
            name: tool.name.clone(),
            description: tool.description.clone(),
            input_schema: tool.parameters.clone(),
        })
        .collect()
}

fn apply_reasoning(request: &mut Request, control: ReasoningControl, effort: ReasoningEffort) {
    match effort {
        ReasoningEffort::Default => {}
        ReasoningEffort::Disabled => request.thinking = Some(Thinking::Disabled),
        effort => {
            // Anthropic rejects custom sampling when thinking is on.
            request.temperature = None;
            request.top_p = None;
            request.thinking = Some(match control {
                ReasoningControl::Effort => Thinking::Adaptive { display: ThinkingDisplay::Summarized },
                ReasoningControl::Budget | ReasoningControl::BudgetAndEffort => {
                    let budget_tokens = budget_tokens(effort);
                    if request.max_tokens <= budget_tokens {
                        request.max_tokens = budget_tokens + 1024;
                    }
                    Thinking::Enabled { budget_tokens }
                }
            });
            let effort = if effort == ReasoningEffort::Minimal { ReasoningEffort::Low } else { effort };
            request.output_config = (control != ReasoningControl::Budget).then_some(OutputConfig { effort });
        }
    }
}

fn budget_tokens(effort: ReasoningEffort) -> u32 {
    match effort {
        ReasoningEffort::Medium => 4096,
        ReasoningEffort::High | ReasoningEffort::Xhigh => 10240,
        ReasoningEffort::Max => 32768,
        // 1024 is the Anthropic API's minimum thinking budget.
        _ => 1024,
    }
}

fn map_assistant_message(content: &str, tool_calls: &[ToolCallRequest], reasoning: &AssistantReasoning) -> Message {
    let has_answer = !content.is_empty() || !tool_calls.is_empty();
    let mut blocks = if has_answer { replay_thinking(reasoning) } else { Vec::new() };
    if !content.is_empty() {
        blocks.push(AnthropicContentBlock::text(content));
    }
    blocks.extend(tool_calls.iter().map(|tool_call| {
        AnthropicContentBlock::ToolUse {
            id: tool_call.id.clone(),
            name: tool_call.name.clone(),
            input: serde_json::from_str(&tool_call.arguments)
                .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new())),
        }
    }));
    Message { role: Role::Assistant, content: blocks }
}

fn replay_thinking(reasoning: &AssistantReasoning) -> Vec<AnthropicContentBlock> {
    let Some(encrypted) = &reasoning.encrypted_content else {
        return Vec::new();
    };
    match serde_json::from_str::<Vec<AnthropicContentBlock>>(&encrypted.content) {
        Ok(blocks) => blocks.into_iter().filter(AnthropicContentBlock::is_thinking).collect(),
        Err(error) => {
            debug!(%error, "Dropping reasoning that is not Messages API thinking");
            Vec::new()
        }
    }
}

fn push_tool_result(messages: &mut Vec<Message>, result: AnthropicContentBlock) {
    match messages.last_mut() {
        Some(Message { role: Role::User, content })
            if content.last().is_some_and(AnthropicContentBlock::is_tool_result) =>
        {
            content.push(result);
        }
        _ => messages.push(Message { role: Role::User, content: vec![result] }),
    }
}

fn map_content_blocks(parts: &[ContentBlock]) -> Result<Vec<AnthropicContentBlock>> {
    parts
        .iter()
        .map(|part| match part {
            ContentBlock::Text { text } => Ok(AnthropicContentBlock::text(text.as_str())),
            ContentBlock::Image { data, mime_type } => Ok(AnthropicContentBlock::Image {
                source: ImageSource {
                    source_type: "base64".to_string(),
                    media_type: mime_type.clone(),
                    data: data.clone(),
                },
                cache_control: None,
            }),
            ContentBlock::Audio { .. } => {
                Err(LlmError::UnsupportedContent("Anthropic does not support audio input".into()))
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MessageId;
    use crate::types::IsoString;
    use serde_json::json;

    #[test]
    fn images_are_sent_as_base64_sources() {
        let context = user_context(vec![
            ContentBlock::text("Look at this:"),
            ContentBlock::Image { data: "aW1n".to_string(), mime_type: "image/png".to_string() },
        ]);

        let request = serde_json::to_value(build_request(None, &context).unwrap()).unwrap();

        assert_eq!(
            request["messages"][0]["content"],
            json!([
                { "type": "text", "text": "Look at this:" },
                {
                    "type": "image",
                    "source": { "type": "base64", "media_type": "image/png", "data": "aW1n" },
                    "cache_control": { "type": "ephemeral" }
                }
            ])
        );
    }

    #[test]
    fn audio_input_is_unsupported() {
        let audio = ContentBlock::Audio { data: "YXVkaW8=".to_string(), mime_type: "audio/wav".to_string() };
        for content in [vec![ContentBlock::text("Listen:"), audio.clone()], vec![audio]] {
            let result = build_request(None, &user_context(content));

            assert!(matches!(result, Err(LlmError::UnsupportedContent(_))));
        }
    }

    #[test]
    fn test_role_enum_serialization() {
        use super::super::types::Role;

        // Test Role::User serialization
        let user_role = Role::User;
        let serialized = serde_json::to_string(&user_role).unwrap();
        assert_eq!(serialized, "\"user\"");

        // Test Role::Assistant serialization
        let assistant_role = Role::Assistant;
        let serialized = serde_json::to_string(&assistant_role).unwrap();
        assert_eq!(serialized, "\"assistant\"");

        // Test Role deserialization
        let user_role: Role = serde_json::from_str("\"user\"").unwrap();
        assert_eq!(user_role, Role::User);

        let assistant_role: Role = serde_json::from_str("\"assistant\"").unwrap();
        assert_eq!(assistant_role, Role::Assistant);
    }

    #[test]
    fn test_cache_type_enum_serialization() {
        use super::super::types::{CacheControl, CacheType};

        // Test CacheType::Ephemeral serialization
        let ephemeral_type = CacheType::Ephemeral;
        let serialized = serde_json::to_string(&ephemeral_type).unwrap();
        assert_eq!(serialized, "\"ephemeral\"");

        // Test CacheType deserialization
        let ephemeral_type: CacheType = serde_json::from_str("\"ephemeral\"").unwrap();
        assert_eq!(ephemeral_type, CacheType::Ephemeral);

        // Test CacheControl struct serialization
        let cache_control = CacheControl::ephemeral();
        let serialized = serde_json::to_string(&cache_control).unwrap();
        assert_eq!(serialized, "{\"type\":\"ephemeral\"}");

        // Test CacheControl struct deserialization
        let cache_control: CacheControl = serde_json::from_str("{\"type\":\"ephemeral\"}").unwrap();
        assert_eq!(cache_control.cache_type, CacheType::Ephemeral);
    }

    fn user_context(content: Vec<ContentBlock>) -> Context {
        Context::new(
            vec![ChatMessage::User { message_id: MessageId::new(), content, timestamp: IsoString::now() }],
            vec![],
        )
    }
}
