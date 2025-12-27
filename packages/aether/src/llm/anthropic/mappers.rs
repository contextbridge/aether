use super::types::{Content, ContentBlock, Message, Role, Tool};
use crate::llm::{ChatMessage, LlmError, Result, ToolDefinition};

pub fn map_messages(messages: &[ChatMessage]) -> Result<(Option<String>, Vec<Message>)> {
    let mut system_prompt = None;
    let mut anthropic_messages = Vec::new();

    for message in messages {
        match message {
            ChatMessage::System { content, .. } => {
                system_prompt = Some(content.clone());
            }
            ChatMessage::User { content, .. } => {
                anthropic_messages.push(Message {
                    role: Role::User,
                    content: Content::Text(content.clone()),
                    cache_control: None,
                });
            }
            ChatMessage::Assistant {
                content,
                tool_calls,
                ..
            } => {
                if tool_calls.is_empty() {
                    anthropic_messages.push(Message {
                        role: Role::Assistant,
                        content: Content::Text(content.clone()),
                        cache_control: None,
                    });
                } else {
                    let mut blocks = if !content.is_empty() {
                        vec![ContentBlock::Text {
                            text: content.clone(),
                            cache_control: None,
                        }]
                    } else {
                        Vec::new()
                    };

                    for tool_call in tool_calls {
                        let input: serde_json::Value = serde_json::from_str(&tool_call.arguments)
                            .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new()));

                        blocks.push(ContentBlock::ToolUse {
                            id: tool_call.id.clone(),
                            name: tool_call.name.clone(),
                            input,
                        });
                    }

                    anthropic_messages.push(Message {
                        role: Role::Assistant,
                        content: Content::Blocks(blocks),
                        cache_control: None,
                    });
                }
            }
            ChatMessage::ToolCallResult(result) => match result {
                Ok(tool_result) => {
                    anthropic_messages.push(Message {
                        role: Role::User,
                        content: Content::Blocks(vec![ContentBlock::ToolResult {
                            tool_use_id: tool_result.id.clone(),
                            content: tool_result.result.clone(),
                            is_error: Some(false),
                        }]),
                        cache_control: None,
                    });
                }
                Err(tool_error) => {
                    anthropic_messages.push(Message {
                        role: Role::User,
                        content: Content::Blocks(vec![ContentBlock::ToolResult {
                            tool_use_id: tool_error.id.clone(),
                            content: tool_error.error.clone(),
                            is_error: Some(true),
                        }]),
                        cache_control: None,
                    });
                }
            },
            ChatMessage::Error { message, .. } => {
                anthropic_messages.push(Message {
                    role: Role::User,
                    content: Content::Text(format!("Error: {message}")),
                    cache_control: None,
                });
            }
            ChatMessage::Summary { content, .. } => {
                // Summaries from previous compaction are treated as user context
                anthropic_messages.push(Message {
                    role: Role::User,
                    content: Content::Text(format!(
                        "[Previous conversation summary]\n\n{content}"
                    )),
                    cache_control: None,
                });
            }
        }
    }

    Ok((system_prompt, anthropic_messages))
}

pub fn map_tools(tools: &[ToolDefinition]) -> Result<Vec<Tool>> {
    let mut anthropic_tools = Vec::new();

    for tool in tools {
        let input_schema: serde_json::Value =
            serde_json::from_str(&tool.parameters).map_err(|e| LlmError::ToolParameterParsing {
                tool_name: tool.name.clone(),
                error: e.to_string(),
            })?;

        anthropic_tools.push(Tool {
            name: tool.name.clone(),
            description: tool.description.clone(),
            input_schema,
            cache_control: None,
        });
    }

    Ok(anthropic_tools)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::tools::ToolCallRequest;
    use crate::types::IsoString;

    #[test]
    fn test_map_simple_user_message() {
        let messages = vec![ChatMessage::User {
            content: "Hello".to_string(),
            timestamp: IsoString::now(),
        }];

        let (system, mapped) = map_messages(&messages).unwrap();
        assert_eq!(system, None);
        assert_eq!(mapped.len(), 1);
        assert_eq!(mapped[0].role, Role::User);
        assert!(matches!(mapped[0].content, Content::Text(_)));
    }

    #[test]
    fn test_map_system_message() {
        let messages = vec![
            ChatMessage::System {
                content: "You are a helpful assistant".to_string(),
                timestamp: IsoString::now(),
            },
            ChatMessage::User {
                content: "Hello".to_string(),
                timestamp: IsoString::now(),
            },
        ];

        let (system, mapped) = map_messages(&messages).unwrap();
        assert_eq!(system, Some("You are a helpful assistant".to_string()));
        assert_eq!(mapped.len(), 1);
    }

    #[test]
    fn test_map_assistant_with_tool_calls() {
        let messages = vec![ChatMessage::Assistant {
            content: "I'll help you with that".to_string(),
            timestamp: IsoString::now(),
            tool_calls: vec![ToolCallRequest {
                id: "call_1".to_string(),
                name: "search".to_string(),
                arguments: r#"{"query": "test"}"#.to_string(),
            }],
        }];

        let (_system, mapped) = map_messages(&messages).unwrap();
        assert_eq!(mapped.len(), 1);
        assert_eq!(mapped[0].role, Role::Assistant);

        if let Content::Blocks(blocks) = &mapped[0].content {
            assert_eq!(blocks.len(), 2);
            assert!(matches!(blocks[0], ContentBlock::Text { .. }));
            assert!(matches!(blocks[1], ContentBlock::ToolUse { .. }));
        } else {
            panic!("Expected blocks content");
        }
    }

    #[test]
    fn test_map_tools() {
        let tools = vec![ToolDefinition {
            name: "search".to_string(),
            description: "Search for information".to_string(),
            parameters: r#"{"type": "object", "properties": {"query": {"type": "string"}}}"#
                .to_string(),
            server: None,
        }];

        let mapped = map_tools(&tools).unwrap();
        assert_eq!(mapped.len(), 1);
        assert_eq!(mapped[0].name, "search");
        assert_eq!(mapped[0].description, "Search for information");
    }

    #[test]
    fn test_map_tools_no_cache_control() {
        let tools = vec![ToolDefinition {
            name: "search".to_string(),
            description: "Search for information".to_string(),
            parameters: r#"{"type": "object", "properties": {"query": {"type": "string"}}}"#
                .to_string(),
            server: None,
        }];

        let mapped = map_tools(&tools).unwrap();
        assert_eq!(mapped.len(), 1);
        // Tools don't have cache_control - they're auto-cached when system prompt is cached
        assert!(mapped[0].cache_control.is_none());
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
}
