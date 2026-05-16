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

use crate::{ChatMessage, ContentBlock, LlmError, Result, ToolCallError, ToolCallResult, ToolDefinition};

fn bedrock_err(e: impl Display) -> LlmError {
    LlmError::Other(e.to_string())
}

pub fn map_messages(
    messages: &[ChatMessage],
    cache_point: Option<&CachePointBlock>,
) -> Result<(Vec<SystemContentBlock>, Vec<Message>)> {
    let mut system_blocks = Vec::new();
    let mut bedrock_messages = Vec::new();
    let mut pending_tool_results = Vec::new();

    for message in messages {
        match message {
            ChatMessage::ToolCallResult(result) => {
                pending_tool_results.push(build_tool_result_block(result)?);
            }

            ChatMessage::System { content, .. } => {
                flush_tool_results(&mut pending_tool_results, &mut bedrock_messages)?;
                system_blocks.push(SystemContentBlock::Text(content.clone()));
            }

            ChatMessage::User { content, .. } => {
                flush_tool_results(&mut pending_tool_results, &mut bedrock_messages)?;
                bedrock_messages.push(build_user_content_blocks(content)?);
            }

            ChatMessage::Assistant { content, tool_calls, .. } => {
                flush_tool_results(&mut pending_tool_results, &mut bedrock_messages)?;
                bedrock_messages.push(map_assistant_message(content, tool_calls)?);
            }

            ChatMessage::Error { message, .. } => {
                flush_tool_results(&mut pending_tool_results, &mut bedrock_messages)?;
                bedrock_messages.push(build_user_message(&format!("Error: {message}"))?);
            }

            ChatMessage::Summary { content, .. } => {
                flush_tool_results(&mut pending_tool_results, &mut bedrock_messages)?;
                bedrock_messages.push(build_user_message(&format!("[Previous conversation handoff]\n\n{content}"))?);
            }
        }
    }

    flush_tool_results(&mut pending_tool_results, &mut bedrock_messages)?;
    if let Some(cache_point) = cache_point {
        if !system_blocks.is_empty() {
            system_blocks.push(SystemContentBlock::CachePoint(cache_point.clone()));
        }
        if let Some(last) = bedrock_messages.pop() {
            let mut builder = Message::builder().role(last.role().clone());
            for block in last.content() {
                builder = builder.content(block.clone());
            }
            builder = builder.content(BedrockContentBlock::CachePoint(cache_point.clone()));
            bedrock_messages.push(builder.build().map_err(bedrock_err)?);
        }
    }

    Ok((system_blocks, bedrock_messages))
}

pub fn map_tools(tools: &[ToolDefinition], cache_point: Option<&CachePointBlock>) -> Result<ToolConfiguration> {
    let mut bedrock_tools: Vec<Tool> = tools
        .iter()
        .map(|tool| {
            let schema_value: serde_json::Value = serde_json::from_str(&tool.parameters)
                .map_err(|e| LlmError::ToolParameterParsing { tool_name: tool.name.clone(), error: e.to_string() })?;
            let spec = ToolSpecification::builder()
                .name(&tool.name)
                .description(&tool.description)
                .input_schema(ToolInputSchema::Json(json_to_document(&schema_value)))
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

pub fn default_cache_point() -> Result<CachePointBlock> {
    CachePointBlock::builder().r#type(CachePointType::Default).build().map_err(bedrock_err)
}

fn build_user_message(content: &str) -> Result<Message> {
    Message::builder()
        .role(ConversationRole::User)
        .content(BedrockContentBlock::Text(content.to_string()))
        .build()
        .map_err(bedrock_err)
}

fn build_user_content_blocks(parts: &[ContentBlock]) -> Result<Message> {
    let mut builder = Message::builder().role(ConversationRole::User);
    for part in parts {
        match part {
            ContentBlock::Text { text } => {
                builder = builder.content(BedrockContentBlock::Text(text.clone()));
            }
            ContentBlock::Image { data, mime_type } => {
                let bytes =
                    BASE64.decode(data).map_err(|e| LlmError::Other(format!("Invalid base64 image data: {e}")))?;
                let format = mime_to_image_format(mime_type);
                builder = builder.content(BedrockContentBlock::Image(
                    ImageBlock::builder()
                        .format(format)
                        .source(ImageSource::Bytes(Blob::new(bytes)))
                        .build()
                        .map_err(bedrock_err)?,
                ));
            }
            ContentBlock::Audio { .. } => {
                return Err(LlmError::UnsupportedContent("Bedrock does not support audio input".into()));
            }
        }
    }
    builder.build().map_err(bedrock_err)
}

fn mime_to_image_format(mime_type: &str) -> ImageFormat {
    match mime_type {
        "image/jpeg" | "image/jpg" => ImageFormat::Jpeg,
        "image/gif" => ImageFormat::Gif,
        "image/webp" => ImageFormat::Webp,
        _ => ImageFormat::Png,
    }
}

fn map_assistant_message(content: &str, tool_calls: &[crate::ToolCallRequest]) -> Result<Message> {
    if tool_calls.is_empty() {
        return Message::builder()
            .role(ConversationRole::Assistant)
            .content(BedrockContentBlock::Text(content.to_string()))
            .build()
            .map_err(bedrock_err);
    }

    let mut builder = Message::builder().role(ConversationRole::Assistant);

    if !content.is_empty() {
        builder = builder.content(BedrockContentBlock::Text(content.to_string()));
    }

    for tool_call in tool_calls {
        let input: serde_json::Value = serde_json::from_str(&tool_call.arguments)
            .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new()));

        let tool_use = ToolUseBlock::builder()
            .tool_use_id(&tool_call.id)
            .name(&tool_call.name)
            .input(json_to_document(&input))
            .build()
            .map_err(bedrock_err)?;

        builder = builder.content(BedrockContentBlock::ToolUse(tool_use));
    }

    builder.build().map_err(bedrock_err)
}

fn flush_tool_results(pending_tool_results: &mut Vec<ToolResultBlock>, messages: &mut Vec<Message>) -> Result<()> {
    if pending_tool_results.is_empty() {
        return Ok(());
    }

    let mut builder = Message::builder().role(ConversationRole::User);
    for tool_result in pending_tool_results.drain(..) {
        builder = builder.content(BedrockContentBlock::ToolResult(tool_result));
    }

    messages.push(builder.build().map_err(bedrock_err)?);
    Ok(())
}

fn build_tool_result_block(result: &result::Result<ToolCallResult, ToolCallError>) -> Result<ToolResultBlock> {
    let (id, content_text, status) = match result {
        Ok(tool_result) => (&tool_result.id, &tool_result.result, ToolResultStatus::Success),
        Err(tool_error) => (&tool_error.id, &tool_error.error, ToolResultStatus::Error),
    };

    ToolResultBlock::builder()
        .tool_use_id(id)
        .content(ToolResultContentBlock::Text(content_text.clone()))
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
    use crate::AssistantReasoning;
    use crate::tools::{ToolCallError, ToolCallRequest, ToolCallResult};
    use crate::types::IsoString;

    fn user_message(text: &str) -> ChatMessage {
        ChatMessage::User { content: vec![ContentBlock::text(text)], timestamp: IsoString::now() }
    }

    fn assistant_message(content: &str, tool_calls: Vec<ToolCallRequest>) -> ChatMessage {
        ChatMessage::Assistant {
            content: content.to_string(),
            reasoning: AssistantReasoning::default(),
            timestamp: IsoString::now(),
            tool_calls,
        }
    }

    fn tool_call(id: &str, name: &str, arguments: &str) -> ToolCallRequest {
        ToolCallRequest { id: id.to_string(), name: name.to_string(), arguments: arguments.to_string() }
    }

    fn tool_result_ok(id: &str, name: &str, arguments: &str, result: &str) -> ChatMessage {
        ChatMessage::ToolCallResult(Ok(ToolCallResult {
            id: id.to_string(),
            name: name.to_string(),
            arguments: arguments.to_string(),
            result: result.to_string(),
        }))
    }

    fn tool_result_err(id: &str, name: &str, error: &str) -> ChatMessage {
        ChatMessage::ToolCallResult(Err(ToolCallError {
            id: id.to_string(),
            name: name.to_string(),
            arguments: None,
            error: error.to_string(),
        }))
    }

    #[test]
    fn test_map_simple_user_message() {
        let messages = vec![user_message("Hello")];

        let (system, mapped) = map_messages(&messages, None).unwrap();
        assert!(system.is_empty());
        assert_eq!(mapped.len(), 1);
        assert_eq!(mapped[0].role(), &ConversationRole::User);
        assert_eq!(mapped[0].content().len(), 1);
        assert!(mapped[0].content()[0].is_text());
    }

    #[test]
    fn test_map_user_message_with_image() {
        let messages = vec![ChatMessage::User {
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
        let messages = vec![
            ChatMessage::System { content: "You are helpful".to_string(), timestamp: IsoString::now() },
            user_message("Hello"),
        ];

        let (system, mapped) = map_messages(&messages, None).unwrap();
        assert_eq!(system.len(), 1);
        assert!(system[0].is_text());
        assert_eq!(mapped.len(), 1);
    }

    #[test]
    fn test_map_assistant_with_tool_calls() {
        let messages =
            vec![assistant_message("I'll help", vec![tool_call("call_1", "search", r#"{"query": "test"}"#)])];

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
        let messages = vec![assistant_message("", vec![tool_call("call_1", "search", r#"{"query": "test"}"#)])];

        let (_system, mapped) = map_messages(&messages, None).unwrap();
        let content = mapped[0].content();
        // Empty text should not be included
        assert_eq!(content.len(), 1);
        assert!(content[0].is_tool_use());
    }

    #[test]
    fn test_map_tool_result_success() {
        let messages = vec![tool_result_ok("call_1", "search", "{}", "found it")];

        let (_system, mapped) = map_messages(&messages, None).unwrap();
        assert_eq!(mapped.len(), 1);
        assert_eq!(mapped[0].role(), &ConversationRole::User);

        let content = mapped[0].content();
        assert_eq!(content.len(), 1);
        assert!(content[0].is_tool_result());
    }

    #[test]
    fn test_map_tool_result_error() {
        let messages = vec![tool_result_err("call_1", "search", "not found")];

        let (_system, mapped) = map_messages(&messages, None).unwrap();
        assert_eq!(mapped.len(), 1);

        let content = mapped[0].content();
        assert_eq!(content.len(), 1);
        assert!(content[0].is_tool_result());
    }

    #[test]
    fn test_map_consecutive_tool_results_into_single_user_message() {
        let messages = vec![
            assistant_message(
                "",
                vec![
                    tool_call("call_1", "find", r#"{"pattern":"**/*.ts"}"#),
                    tool_call("call_2", "find", r#"{"pattern":"**/package.json"}"#),
                ],
            ),
            tool_result_ok("call_1", "find", r#"{"pattern":"**/*.ts"}"#, "17 files"),
            tool_result_ok("call_2", "find", r#"{"pattern":"**/package.json"}"#, "2 files"),
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
        let messages = vec![ChatMessage::Error { message: "something broke".to_string(), timestamp: IsoString::now() }];

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
        let tools = vec![ToolDefinition {
            name: "search".to_string(),
            description: "Search for information".to_string(),
            parameters: r#"{"type": "object", "properties": {"query": {"type": "string"}}}"#.to_string(),
            server: None,
        }];

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
        let messages = vec![
            ChatMessage::System { content: "You are helpful".to_string(), timestamp: IsoString::now() },
            user_message("Hello"),
        ];
        let cache_point = default_cache_point().unwrap();

        let (system, _mapped) = map_messages(&messages, Some(&cache_point)).unwrap();

        assert_eq!(system.len(), 2);
        assert!(system[0].is_text());
        assert!(system[1].is_cache_point());
    }

    #[test]
    fn system_cache_point_is_not_added_without_system_content() {
        let messages = vec![user_message("Hello")];
        let cache_point = default_cache_point().unwrap();

        let (system, _mapped) = map_messages(&messages, Some(&cache_point)).unwrap();

        assert!(system.is_empty());
    }

    #[test]
    fn message_cache_point_is_added_to_last_user_message() {
        let messages = vec![user_message("Hello")];
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
        let messages = vec![user_message("turn 1"), assistant_message("ack", vec![]), user_message("turn 2")];
        let cache_point = default_cache_point().unwrap();

        let (_system, mapped) = map_messages(&messages, Some(&cache_point)).unwrap();

        assert!(mapped[0].content().iter().all(|b| !b.is_cache_point()));
        assert!(mapped[1].content().iter().all(|b| !b.is_cache_point()));
        assert!(mapped[2].content().last().unwrap().is_cache_point());
    }

    #[test]
    fn message_cache_point_is_added_to_tool_result_message() {
        let messages = vec![
            user_message("search please"),
            assistant_message("", vec![tool_call("call_1", "search", "{}")]),
            tool_result_ok("call_1", "search", "{}", "found"),
        ];
        let cache_point = default_cache_point().unwrap();
        let (_system, mapped) = map_messages(&messages, Some(&cache_point)).unwrap();

        let last = mapped.last().unwrap();
        assert_eq!(last.role(), &ConversationRole::User);
        assert!(last.content().last().unwrap().is_cache_point());
    }

    #[test]
    fn message_cache_points_not_added_when_cache_point_is_none() {
        let messages = vec![user_message("turn 1"), assistant_message("ack", vec![]), user_message("turn 2")];

        let (_system, mapped) = map_messages(&messages, None).unwrap();

        for msg in &mapped {
            assert!(msg.content().iter().all(|b| !b.is_cache_point()));
        }
    }

    #[test]
    fn tool_cache_point_is_added_when_cache_point_provided() {
        let tools = vec![ToolDefinition {
            name: "search".to_string(),
            description: "Search for information".to_string(),
            parameters: r#"{"type": "object", "properties": {"query": {"type": "string"}}}"#.to_string(),
            server: None,
        }];
        let cache_point = default_cache_point().unwrap();

        let config = map_tools(&tools, Some(&cache_point)).unwrap();

        assert_eq!(config.tools().len(), 2);
        assert!(config.tools()[0].is_tool_spec());
        assert!(config.tools()[1].is_cache_point());
    }

    #[test]
    fn test_map_tools_invalid_json() {
        let tools = vec![ToolDefinition {
            name: "broken".to_string(),
            description: "A broken tool".to_string(),
            parameters: "not valid json".to_string(),
            server: None,
        }];

        let result = map_tools(&tools, None);
        assert!(result.is_err());
        match result.unwrap_err() {
            LlmError::ToolParameterParsing { tool_name, .. } => {
                assert_eq!(tool_name, "broken");
            }
            other => panic!("Expected ToolParameterParsing, got {other:?}"),
        }
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
