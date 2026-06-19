use async_openai::types::responses::{
    EasyInputContent, EasyInputMessage, FunctionCallOutput, FunctionCallOutputItemParam, FunctionTool,
    FunctionToolCall, InputItem, Item, MessageType, ReasoningItem, Role, Tool,
};

use crate::providers::openai::responses_provider::map_user_content_for_responses;
use crate::{ChatMessage, LlmError, Result, ToolDefinition};

/// Map internal `ChatMessage`s to Codex Responses API input items.
///
/// Returns `(system_prompt, input_items)` — the system prompt is extracted
/// separately since the Codex API uses `instructions` for it.
pub fn map_messages(messages: &[ChatMessage]) -> crate::Result<(Option<String>, Vec<InputItem>)> {
    let mut system_prompt = None;
    let mut items = Vec::new();

    for msg in messages {
        match msg {
            ChatMessage::System { content, .. } => {
                system_prompt = Some(content.clone());
            }
            ChatMessage::User { content, .. } => {
                items.push(InputItem::EasyMessage(EasyInputMessage {
                    r#type: MessageType::Message,
                    role: Role::User,
                    content: map_user_content_for_responses(content)?,
                    phase: None,
                }));
            }
            ChatMessage::Assistant { content, tool_calls, reasoning, .. } => {
                if !content.is_empty() {
                    items.push(easy_message(Role::Assistant, content.clone()));
                }
                if let Some(encrypted) = &reasoning.encrypted_content {
                    items.push(InputItem::Item(Item::Reasoning(ReasoningItem {
                        id: Some(encrypted.id.clone()),
                        summary: vec![],
                        encrypted_content: Some(encrypted.content.clone()),
                        content: None,
                        status: None,
                    })));
                }
                for tc in tool_calls {
                    items.push(InputItem::Item(Item::FunctionCall(FunctionToolCall {
                        call_id: tc.id.clone(),
                        name: tc.name.clone(),
                        arguments: tc.arguments.clone(),
                        namespace: None,
                        id: None,
                        status: None,
                    })));
                }
            }
            ChatMessage::ToolCallResult(result) => match result {
                Ok(r) => {
                    items.push(InputItem::Item(Item::FunctionCallOutput(FunctionCallOutputItemParam {
                        call_id: r.id.clone(),
                        output: FunctionCallOutput::Text(r.result.clone()),
                        id: None,
                        status: None,
                    })));
                }
                Err(e) => {
                    items.push(InputItem::Item(Item::FunctionCallOutput(FunctionCallOutputItemParam {
                        call_id: e.id.clone(),
                        output: FunctionCallOutput::Text(format!("Error: {}", e.error)),
                        id: None,
                        status: None,
                    })));
                }
            },
            ChatMessage::Error { message, .. } => {
                items.push(easy_message(Role::User, format!("[Error: {message}]")));
            }
            ChatMessage::Summary { content, .. } => {
                items.push(easy_message(Role::User, format!("[Summary of previous conversation]\n{content}")));
            }
        }
    }

    Ok((system_prompt, items))
}

/// Map internal `ToolDefinition`s to async-openai `Tool` types.
pub fn map_tools(tools: &[ToolDefinition]) -> Result<Vec<Tool>> {
    tools
        .iter()
        .map(|tool| {
            let parameters: serde_json::Value = serde_json::from_str(&tool.parameters)
                .map_err(|e| LlmError::ToolParameterParsing { tool_name: tool.name.clone(), error: e.to_string() })?;

            Ok(Tool::Function(FunctionTool {
                name: tool.name.clone(),
                description: Some(tool.description.clone()),
                parameters: Some(parameters),
                strict: None,
                defer_loading: None,
            }))
        })
        .collect()
}

fn easy_message(role: Role, content: String) -> InputItem {
    InputItem::EasyMessage(EasyInputMessage { role, content: EasyInputContent::Text(content), ..Default::default() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::IsoString;
    use crate::{
        AssistantReasoning, ContentBlock, EncryptedReasoningContent, ToolCallError, ToolCallRequest, ToolCallResult,
    };

    #[test]
    fn map_messages_extracts_system_prompt() {
        let messages = vec![
            ChatMessage::System { content: "You are helpful".to_string(), timestamp: IsoString::now() },
            ChatMessage::User { content: vec![ContentBlock::text("Hello")], timestamp: IsoString::now() },
        ];

        let (system, items) = map_messages(&messages).unwrap();
        assert_eq!(system, Some("You are helpful".to_string()));
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn map_messages_handles_multi_turn_with_tool_calls() {
        let messages = vec![
            ChatMessage::User { content: vec![ContentBlock::text("Read foo.rs")], timestamp: IsoString::now() },
            ChatMessage::Assistant {
                content: "I'll read that file.".to_string(),
                reasoning: AssistantReasoning::default(),
                timestamp: IsoString::now(),
                tool_calls: vec![ToolCallRequest {
                    id: "call_1".to_string(),
                    name: "read_file".to_string(),
                    arguments: r#"{"path":"foo.rs"}"#.to_string(),
                }],
            },
            ChatMessage::ToolCallResult(Ok(ToolCallResult {
                id: "call_1".to_string(),
                name: "read_file".to_string(),
                arguments: r#"{"path":"foo.rs"}"#.to_string(),
                result: "fn main() {}".to_string(),
            })),
            ChatMessage::Assistant {
                content: "Here's the file content.".to_string(),
                reasoning: AssistantReasoning::default(),
                timestamp: IsoString::now(),
                tool_calls: vec![],
            },
        ];

        let (system, items) = map_messages(&messages).unwrap();
        assert!(system.is_none());
        assert_eq!(items.len(), 5); // user + assistant msg + function_call + function_call_output + assistant msg

        // Verify the function_call item
        let fc = &items[2];
        if let InputItem::Item(Item::FunctionCall(call)) = fc {
            assert_eq!(call.call_id, "call_1");
            assert_eq!(call.name, "read_file");
            assert_eq!(call.arguments, r#"{"path":"foo.rs"}"#);
        } else {
            panic!("Expected FunctionCall, got {fc:?}");
        }

        // Verify the function_call_output item
        let fco = &items[3];
        if let InputItem::Item(Item::FunctionCallOutput(out)) = fco {
            assert_eq!(out.call_id, "call_1");
            assert!(matches!(&out.output, FunctionCallOutput::Text(t) if t == "fn main() {}"));
        } else {
            panic!("Expected FunctionCallOutput, got {fco:?}");
        }
    }

    #[test]
    fn map_messages_handles_tool_errors() {
        let messages = vec![ChatMessage::ToolCallResult(Err(ToolCallError {
            id: "call_2".to_string(),
            name: "bash".to_string(),
            arguments: Some("{}".to_string()),
            error: "command failed".to_string(),
        }))];

        let (_, items) = map_messages(&messages).unwrap();
        assert_eq!(items.len(), 1);
        if let InputItem::Item(Item::FunctionCallOutput(out)) = &items[0] {
            assert!(matches!(&out.output, FunctionCallOutput::Text(t) if t.contains("Error: command failed")));
        } else {
            panic!("Expected FunctionCallOutput");
        }
    }

    #[test]
    fn map_messages_handles_summary() {
        let messages = vec![ChatMessage::Summary {
            content: "User asked about Rust.".to_string(),
            timestamp: IsoString::now(),
            messages_compacted: 5,
        }];

        let (_, items) = map_messages(&messages).unwrap();
        assert_eq!(items.len(), 1);
        if let InputItem::EasyMessage(msg) = &items[0] {
            assert_eq!(msg.role, Role::User);
            if let EasyInputContent::Text(text) = &msg.content {
                assert!(text.contains("Summary"));
                assert!(text.contains("Rust"));
            } else {
                panic!("Expected Text content");
            }
        } else {
            panic!("Expected EasyMessage");
        }
    }

    #[test]
    fn map_messages_serialization_shape() {
        let messages = vec![
            ChatMessage::User { content: vec![ContentBlock::text("Hello")], timestamp: IsoString::now() },
            ChatMessage::Assistant {
                content: "Hi".to_string(),
                reasoning: AssistantReasoning::default(),
                timestamp: IsoString::now(),
                tool_calls: vec![ToolCallRequest {
                    id: "tc_1".to_string(),
                    name: "bash".to_string(),
                    arguments: "{}".to_string(),
                }],
            },
            ChatMessage::ToolCallResult(Ok(ToolCallResult {
                id: "tc_1".to_string(),
                name: "bash".to_string(),
                arguments: "{}".to_string(),
                result: "ok".to_string(),
            })),
        ];

        let (_, items) = map_messages(&messages).unwrap();
        // EasyMessage items serialize with "type": "message"
        let json = serde_json::to_value(&items[0]).unwrap();
        assert_eq!(json["role"], "user");
        // FunctionCall items serialize with "type": "function_call"
        let json = serde_json::to_value(&items[2]).unwrap();
        assert_eq!(json["type"], "function_call");
        assert_eq!(json["call_id"], "tc_1");
        // FunctionCallOutput items serialize with "type": "function_call_output"
        let json = serde_json::to_value(&items[3]).unwrap();
        assert_eq!(json["type"], "function_call_output");
        assert_eq!(json["call_id"], "tc_1");
    }

    #[test]
    fn map_tools_produces_function_type() {
        let tools = vec![ToolDefinition::new(
            "read_file",
            "Read a file from disk",
            r#"{"type": "object", "properties": {"path": {"type": "string"}}}"#,
        )];

        let mapped = map_tools(&tools).unwrap();
        assert_eq!(mapped.len(), 1);
        if let Tool::Function(f) = &mapped[0] {
            assert_eq!(f.name, "read_file");
            assert_eq!(f.description.as_deref(), Some("Read a file from disk"));
            assert_eq!(f.parameters.as_ref().unwrap()["properties"]["path"]["type"], "string");
        } else {
            panic!("Expected Tool::Function");
        }
    }

    #[test]
    fn map_tools_returns_error_on_invalid_json_parameters() {
        let tools = vec![ToolDefinition::new("broken", "A tool with invalid params", "not valid json")];

        let result = map_tools(&tools);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, LlmError::ToolParameterParsing { ref tool_name, .. } if tool_name == "broken"));
    }

    #[test]
    fn map_messages_includes_encrypted_reasoning_item() {
        let messages = vec![ChatMessage::Assistant {
            content: "thinking done".to_string(),
            reasoning: AssistantReasoning::from_parts(
                "summary".to_string(),
                Some(EncryptedReasoningContent {
                    id: "r_1".to_string(),
                    model: crate::LlmModel::Ollama("test".to_string()),
                    content: "encrypted-blob".to_string(),
                }),
            ),
            timestamp: IsoString::now(),
            tool_calls: vec![],
        }];

        let (_, items) = map_messages(&messages).unwrap();
        // Should have: easy_message (text) + reasoning item = 2
        assert_eq!(items.len(), 2);

        let reasoning_item = &items[1];
        if let InputItem::Item(Item::Reasoning(r)) = reasoning_item {
            assert_eq!(r.encrypted_content.as_deref(), Some("encrypted-blob"));
        } else {
            panic!("Expected Item::Reasoning, got {reasoning_item:?}");
        }
    }

    #[test]
    fn map_messages_skips_reasoning_item_without_encrypted_content() {
        let messages = vec![ChatMessage::Assistant {
            content: "no encrypted".to_string(),
            reasoning: AssistantReasoning::from_parts("just a summary".to_string(), None),
            timestamp: IsoString::now(),
            tool_calls: vec![],
        }];

        let (_, items) = map_messages(&messages).unwrap();
        // Only the text message, no reasoning item
        assert_eq!(items.len(), 1);
        assert!(matches!(&items[0], InputItem::EasyMessage(_)));
    }

    #[test]
    fn map_messages_with_audio_errors() {
        let messages = vec![ChatMessage::User {
            content: vec![ContentBlock::Audio { data: "YXVkaW8=".to_string(), mime_type: "audio/wav".to_string() }],
            timestamp: IsoString::now(),
        }];

        assert!(matches!(map_messages(&messages), Err(LlmError::UnsupportedContent(_))));
    }
}
