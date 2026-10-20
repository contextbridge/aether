use aws_sdk_bedrockruntime::types::{
    CachePointBlock, CachePointType, ContentBlock as BedrockContentBlock, ConversationRole, ImageBlock, ImageFormat,
    ImageSource, Message, SystemContentBlock, Tool, ToolConfiguration, ToolInputSchema, ToolResultBlock,
    ToolResultContentBlock, ToolResultStatus, ToolSpecification, ToolUseBlock,
};
use aws_smithy_types::{Blob, Document, Number};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde_json::Value;
use std::{collections::HashMap, fmt::Display, result};

use crate::providers::turns::{Speaker, Turns};
use crate::{
    AssistantContent, AssistantReply, ChatMessage, ContentBlock, LlmError, Result, ToolCallError, ToolCallRequest,
    ToolCallResult, ToolDefinition, tool_result_parts,
};

fn bedrock_err(e: impl Display) -> LlmError {
    LlmError::ProviderRequest(e.to_string())
}

pub(super) fn map_messages(
    messages: &[ChatMessage],
    cache_point: Option<&CachePointBlock>,
) -> Result<(Vec<SystemContentBlock>, Vec<Message>)> {
    let mut system = Vec::new();
    let mut turns = Turns::default();

    for message in messages {
        match message {
            ChatMessage::System { content, .. } => system.push(SystemContentBlock::Text(content.clone())),
            ChatMessage::User { content, .. } => turns.user(map_user_content(content)?),
            ChatMessage::Assistant { content, .. } => turns.assistant(map_assistant_content(content)?),
            ChatMessage::ToolCallResult(result) => {
                turns.tool_result(BedrockContentBlock::ToolResult(build_tool_result_block(result)?));
            }
            ChatMessage::Error { .. } | ChatMessage::Summary { .. } => {
                if let Some(note) = message.as_user_note() {
                    turns.user(vec![BedrockContentBlock::Text(note)]);
                }
            }
        }
    }

    if let Some(cache_point) = cache_point {
        if !system.is_empty() {
            system.push(SystemContentBlock::CachePoint(cache_point.clone()));
        }
        if let Some(blocks) = turns.last_turn_mut() {
            blocks.push(BedrockContentBlock::CachePoint(cache_point.clone()));
        }
    }

    let messages = turns
        .into_turns()
        .map(|(speaker, content)| {
            let role = match speaker {
                Speaker::User => ConversationRole::User,
                Speaker::Assistant => ConversationRole::Assistant,
            };
            Message::builder().role(role).set_content(Some(content)).build().map_err(bedrock_err)
        })
        .collect::<Result<_>>()?;
    Ok((system, messages))
}

pub(super) fn map_tools(tools: &[ToolDefinition], cache_point: Option<&CachePointBlock>) -> Result<ToolConfiguration> {
    let mut bedrock_tools: Vec<Tool> = tools
        .iter()
        .map(|tool| {
            let spec = ToolSpecification::builder()
                .name(&tool.name)
                .description(&tool.description)
                .input_schema(ToolInputSchema::Json(json_to_document(&tool.parameters)))
                .build()
                .map_err(bedrock_err)?;
            Ok(Tool::ToolSpec(spec))
        })
        .collect::<Result<_>>()?;

    if let Some(cache_point) = cache_point
        && !bedrock_tools.is_empty()
    {
        bedrock_tools.push(Tool::CachePoint(cache_point.clone()));
    }

    ToolConfiguration::builder().set_tools(Some(bedrock_tools)).build().map_err(bedrock_err)
}

pub(super) fn default_cache_point() -> Result<CachePointBlock> {
    CachePointBlock::builder().r#type(CachePointType::Default).build().map_err(bedrock_err)
}

fn map_user_content(parts: &[ContentBlock]) -> Result<Vec<BedrockContentBlock>> {
    parts
        .iter()
        .map(|part| match part {
            ContentBlock::Text { text } => Ok(BedrockContentBlock::Text(text.clone())),
            ContentBlock::Image { data, mime_type } => {
                let bytes = BASE64
                    .decode(data)
                    .map_err(|e| LlmError::ProviderRequest(format!("Invalid base64 image data: {e}")))?;
                let image = ImageBlock::builder()
                    .format(mime_to_image_format(mime_type))
                    .source(ImageSource::Bytes(Blob::new(bytes)))
                    .build()
                    .map_err(bedrock_err)?;
                Ok(BedrockContentBlock::Image(image))
            }
            ContentBlock::Audio { .. } => {
                Err(LlmError::UnsupportedContent("Bedrock does not support audio input".into()))
            }
        })
        .collect()
}

fn mime_to_image_format(mime_type: &str) -> ImageFormat {
    match mime_type {
        "image/jpeg" | "image/jpg" => ImageFormat::Jpeg,
        "image/gif" => ImageFormat::Gif,
        "image/webp" => ImageFormat::Webp,
        _ => ImageFormat::Png,
    }
}

fn map_assistant_content(reply: &AssistantReply) -> Result<Vec<BedrockContentBlock>> {
    reply
        .blocks()
        .iter()
        .filter_map(|block| match block {
            AssistantContent::Text { text } => (!text.is_empty()).then(|| Ok(BedrockContentBlock::Text(text.clone()))),
            AssistantContent::ToolCall(call) => Some(map_tool_use(call)),
            AssistantContent::Reasoning { .. } => None,
        })
        .collect()
}

fn map_tool_use(call: &ToolCallRequest) -> Result<BedrockContentBlock> {
    let tool_use = ToolUseBlock::builder()
        .tool_use_id(&call.id)
        .name(&call.name)
        .input(json_to_document(&call.input_json()))
        .build()
        .map_err(bedrock_err)?;
    Ok(BedrockContentBlock::ToolUse(tool_use))
}

fn build_tool_result_block(result: &result::Result<ToolCallResult, ToolCallError>) -> Result<ToolResultBlock> {
    let (id, content_text, is_error) = tool_result_parts(result);
    let status = if is_error { ToolResultStatus::Error } else { ToolResultStatus::Success };

    ToolResultBlock::builder()
        .tool_use_id(id)
        .content(ToolResultContentBlock::Text(content_text.to_string()))
        .status(status)
        .build()
        .map_err(bedrock_err)
}

fn json_to_document(value: &Value) -> Document {
    match value {
        Value::Null => Document::Null,
        Value::Bool(b) => Document::Bool(*b),
        Value::Number(n) => {
            if let Some(u) = n.as_u64() {
                Document::Number(Number::PosInt(u))
            } else if let Some(i) = n.as_i64() {
                Document::Number(Number::NegInt(i))
            } else if let Some(f) = n.as_f64() {
                Document::Number(Number::Float(f))
            } else {
                Document::Null
            }
        }
        Value::String(s) => Document::String(s.clone()),
        Value::Array(arr) => Document::Array(arr.iter().map(json_to_document).collect()),
        Value::Object(obj) => {
            let map: HashMap<String, Document> = obj.iter().map(|(k, v)| (k.clone(), json_to_document(v))).collect();
            Document::Object(map)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MessageId;
    use crate::testing::{assistant_message, tool_error, tool_result};
    use crate::types::IsoString;

    #[test]
    fn test_map_simple_user_message() {
        let messages = vec![ChatMessage::user("Hello")];

        let (system, mapped) = map_messages(&messages, None).unwrap();
        assert_eq!(system.len(), 0);
        assert_eq!(mapped.len(), 1);
        assert_eq!(mapped[0].role(), &ConversationRole::User);
        assert_eq!(mapped[0].content().len(), 1);
        assert!(mapped[0].content()[0].is_text());
    }

    #[test]
    fn test_map_user_message_with_image() {
        let messages = vec![ChatMessage::User {
            message_id: MessageId::new(),
            content: vec![
                ContentBlock::text("Look:"),
                ContentBlock::Image { data: BASE64.encode(b"fakepng"), mime_type: "image/png".to_string() },
            ],
            timestamp: IsoString::now(),
        }];

        let (_system, mapped) = map_messages(&messages, None).unwrap();
        assert_eq!(mapped[0].content().len(), 2);
        assert!(mapped[0].content()[0].is_text());
        assert!(mapped[0].content()[1].is_image());
    }

    #[test]
    fn test_map_user_message_with_audio_errors() {
        let messages = vec![ChatMessage::User {
            message_id: MessageId::new(),
            content: vec![
                ContentBlock::text("Listen:"),
                ContentBlock::Audio { data: BASE64.encode(b"fakewav"), mime_type: "audio/wav".to_string() },
            ],
            timestamp: IsoString::now(),
        }];

        assert!(matches!(map_messages(&messages, None), Err(LlmError::UnsupportedContent(_))));
    }

    #[test]
    fn test_map_system_message() {
        let messages = vec![ChatMessage::system("You are helpful"), ChatMessage::user("Hello")];

        let (system, mapped) = map_messages(&messages, None).unwrap();
        assert_eq!(system.len(), 1);
        assert!(system[0].is_text());
        assert_eq!(mapped.len(), 1);
    }

    #[test]
    fn test_map_assistant_with_tool_calls() {
        let messages =
            vec![assistant_message().text("I'll help").tool_call("call_1", "search", r#"{"query": "test"}"#).build()];

        let (_system, mapped) = map_messages(&messages, None).unwrap();
        assert_eq!(mapped.len(), 1);
        assert_eq!(mapped[0].role(), &ConversationRole::Assistant);

        let content = mapped[0].content();
        assert_eq!(content.len(), 2);
        assert!(content[0].is_text());
        assert!(content[1].is_tool_use());
    }

    #[test]
    fn test_map_assistant_tool_calls_without_text() {
        let messages = vec![assistant_message().tool_call("call_1", "search", r#"{"query": "test"}"#).build()];

        let (_system, mapped) = map_messages(&messages, None).unwrap();
        let content = mapped[0].content();
        // Empty text should not be included
        assert_eq!(content.len(), 1);
        assert!(content[0].is_tool_use());
    }

    #[test]
    fn test_map_tool_result_success() {
        let messages = vec![tool_result("call_1", "search", "{}", "found it")];

        let (_system, mapped) = map_messages(&messages, None).unwrap();
        assert_eq!(mapped.len(), 1);
        assert_eq!(mapped[0].role(), &ConversationRole::User);

        let content = mapped[0].content();
        assert_eq!(content.len(), 1);
        assert!(content[0].is_tool_result());
    }

    #[test]
    fn test_map_tool_result_error() {
        let messages = vec![tool_error("call_1", "search", "not found")];

        let (_system, mapped) = map_messages(&messages, None).unwrap();
        assert_eq!(mapped.len(), 1);

        let content = mapped[0].content();
        assert_eq!(content.len(), 1);
        assert!(content[0].is_tool_result());
    }

    #[test]
    fn test_map_consecutive_tool_results_into_single_user_message() {
        let messages = vec![
            assistant_message()
                .tool_call("call_1", "find", r#"{"pattern":"**/*.ts"}"#)
                .tool_call("call_2", "find", r#"{"pattern":"**/package.json"}"#)
                .build(),
            tool_result("call_1", "find", r#"{"pattern":"**/*.ts"}"#, "17 files"),
            tool_result("call_2", "find", r#"{"pattern":"**/package.json"}"#, "2 files"),
        ];

        let (_system, mapped) = map_messages(&messages, None).unwrap();
        assert_eq!(mapped.len(), 2);
        assert_eq!(mapped[0].role(), &ConversationRole::Assistant);
        assert_eq!(mapped[1].role(), &ConversationRole::User);
        assert_eq!(mapped[0].content().len(), 2);
        assert!(mapped[0].content().iter().all(BedrockContentBlock::is_tool_use));

        let tool_results = mapped[1].content();
        assert_eq!(tool_results.len(), 2);
        assert!(tool_results.iter().all(BedrockContentBlock::is_tool_result));
    }

    #[test]
    fn test_map_error_message() {
        let messages = vec![ChatMessage::Error {
            message_id: MessageId::new(),
            message: "something broke".to_string(),
            timestamp: IsoString::now(),
        }];

        let (_system, mapped) = map_messages(&messages, None).unwrap();
        assert_eq!(mapped.len(), 1);
        assert_eq!(mapped[0].role(), &ConversationRole::User);
        match &mapped[0].content()[0] {
            BedrockContentBlock::Text(text) => assert!(text.contains("something broke")),
            other => panic!("Expected text, got {other:?}"),
        }
    }

    #[test]
    fn test_map_summary_message() {
        let messages = vec![ChatMessage::Summary {
            message_id: MessageId::new(),
            content: "we talked about stuff".to_string(),
            timestamp: IsoString::now(),
            messages_compacted: 10,
        }];

        let (_system, mapped) = map_messages(&messages, None).unwrap();
        assert_eq!(mapped.len(), 1);
        match &mapped[0].content()[0] {
            BedrockContentBlock::Text(text) => {
                assert!(text.contains("[Previous conversation handoff]"));
                assert!(text.contains("we talked about stuff"));
            }
            other => panic!("Expected text, got {other:?}"),
        }
    }

    #[test]
    fn test_map_tools() {
        let tools = vec![ToolDefinition::new(
            "search",
            "Search for information",
            serde_json::from_str(r#"{"type": "object", "properties": {"query": {"type": "string"}}}"#).unwrap(),
        )];

        let config = map_tools(&tools, None).unwrap();
        assert_eq!(config.tools().len(), 1);
        match &config.tools()[0] {
            Tool::ToolSpec(spec) => {
                assert_eq!(spec.name(), "search");
                assert_eq!(spec.description(), Some("Search for information"));
            }
            other => panic!("Expected ToolSpec, got {other:?}"),
        }
    }

    #[test]
    fn system_cache_point_is_added_when_cache_point_provided() {
        let messages = vec![ChatMessage::system("You are helpful"), ChatMessage::user("Hello")];
        let cache_point = default_cache_point().unwrap();

        let (system, _mapped) = map_messages(&messages, Some(&cache_point)).unwrap();

        assert_eq!(system.len(), 2);
        assert!(system[0].is_text());
        assert!(system[1].is_cache_point());
    }

    #[test]
    fn system_cache_point_is_not_added_without_system_content() {
        let messages = vec![ChatMessage::user("Hello")];
        let cache_point = default_cache_point().unwrap();

        let (system, _mapped) = map_messages(&messages, Some(&cache_point)).unwrap();

        assert_eq!(system.len(), 0);
    }

    #[test]
    fn message_cache_point_is_added_to_last_user_message() {
        let messages = vec![ChatMessage::user("Hello")];
        let cache_point = default_cache_point().unwrap();

        let (_system, mapped) = map_messages(&messages, Some(&cache_point)).unwrap();

        assert_eq!(mapped.len(), 1);
        let content = mapped[0].content();
        assert_eq!(content.len(), 2);
        assert!(content[0].is_text());
        assert!(content[1].is_cache_point());
    }

    #[test]
    fn message_cache_point_is_added_only_to_last_message_in_multi_turn_history() {
        let messages =
            vec![ChatMessage::user("turn 1"), assistant_message().text("ack").build(), ChatMessage::user("turn 2")];
        let cache_point = default_cache_point().unwrap();

        let (_system, mapped) = map_messages(&messages, Some(&cache_point)).unwrap();

        assert!(mapped[0].content().iter().all(|b| !b.is_cache_point()));
        assert!(mapped[1].content().iter().all(|b| !b.is_cache_point()));
        assert!(mapped[2].content().last().unwrap().is_cache_point());
    }

    #[test]
    fn message_cache_point_is_added_to_tool_result_message() {
        let messages = vec![
            ChatMessage::user("search please"),
            assistant_message().tool_call("call_1", "search", "{}").build(),
            tool_result("call_1", "search", "{}", "found"),
        ];
        let cache_point = default_cache_point().unwrap();
        let (_system, mapped) = map_messages(&messages, Some(&cache_point)).unwrap();

        let last = mapped.last().unwrap();
        assert_eq!(last.role(), &ConversationRole::User);
        assert!(last.content().last().unwrap().is_cache_point());
    }

    #[test]
    fn message_cache_points_not_added_when_cache_point_is_none() {
        let messages =
            vec![ChatMessage::user("turn 1"), assistant_message().text("ack").build(), ChatMessage::user("turn 2")];

        let (_system, mapped) = map_messages(&messages, None).unwrap();

        for msg in &mapped {
            assert!(msg.content().iter().all(|b| !b.is_cache_point()));
        }
    }

    #[test]
    fn tool_cache_point_is_added_when_cache_point_provided() {
        let tools = vec![ToolDefinition::new(
            "search",
            "Search for information",
            serde_json::from_str(r#"{"type": "object", "properties": {"query": {"type": "string"}}}"#).unwrap(),
        )];
        let cache_point = default_cache_point().unwrap();

        let config = map_tools(&tools, Some(&cache_point)).unwrap();

        assert_eq!(config.tools().len(), 2);
        assert!(config.tools()[0].is_tool_spec());
        assert!(config.tools()[1].is_cache_point());
    }

    #[test]
    fn test_json_to_document_primitives() {
        assert_eq!(json_to_document(&serde_json::Value::Null), Document::Null);
        assert_eq!(json_to_document(&serde_json::Value::Bool(true)), Document::Bool(true));
        assert_eq!(
            json_to_document(&serde_json::Value::String("hello".to_string())),
            Document::String("hello".to_string())
        );
    }

    #[test]
    fn test_json_to_document_nested_object() {
        let json: serde_json::Value =
            serde_json::from_str(r#"{"type": "object", "properties": {"name": {"type": "string"}}}"#).unwrap();

        let doc = json_to_document(&json);
        match &doc {
            Document::Object(map) => {
                assert!(map.contains_key("type"));
                assert!(map.contains_key("properties"));
            }
            other => panic!("Expected Object, got {other:?}"),
        }
    }
}
