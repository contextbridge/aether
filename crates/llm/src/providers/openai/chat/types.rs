use serde::{Deserialize, Serialize};

use crate::providers::openai::wire::Usage;
use crate::{ChatMessage, ContentBlock, tool_result_parts};

/// Unified custom types for OpenAI-compatible APIs that deviate slightly from the standard.
/// This handles quirks from providers like `OpenRouter`, Z.ai, and potentially others.
///
/// Common deviations handled:
/// - Negative token counts (openrouter)
/// - Additional finish reasons like 'error' (openrouter)
/// - Optional usage fields

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FinishReason {
    Stop,
    Length,
    ToolCalls,
    ContentFilter,
    FunctionCall,
    Error,
    NetworkError,
    ModelContextWindowExceeded,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ChatCompletionStreamResponse {
    pub choices: Vec<ChatCompletionStreamChoice>,
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub(crate) enum UserContent {
    Text(String),
    Parts(Vec<UserContentPart>),
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum UserContentPart {
    Text { text: String },
    ImageUrl { image_url: ImageUrlContent },
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ImageUrlContent {
    pub url: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "role", rename_all = "lowercase")]
pub(crate) enum CompatibleChatMessage {
    System {
        content: String,
    },
    User {
        content: UserContent,
    },
    Assistant {
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        reasoning_content: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_calls: Option<Vec<ToolCall>>,
    },
    Tool {
        content: String,
        tool_call_id: String,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ToolCall {
    Function { id: String, function: FunctionCall },
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum Tool {
    Function { function: FunctionDefinition },
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct FunctionDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    pub strict: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct StreamOptions {
    pub include_usage: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct CompatibleChatRequest {
    pub model: String,
    pub messages: Vec<CompatibleChatMessage>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Tool>>,
    pub stream_options: StreamOptions,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// OpenAI-style prompt-cache affinity. Only set for providers that document support.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_cache_key: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

pub(crate) fn map_messages(
    messages: &[ChatMessage],
    tool_call_reasoning_placeholder: bool,
) -> crate::Result<Vec<CompatibleChatMessage>> {
    let mut result = Vec::new();

    for message in messages {
        let mapped = match message {
            ChatMessage::System { content, .. } => Some(CompatibleChatMessage::System { content: content.clone() }),
            ChatMessage::User { content, .. } => {
                Some(CompatibleChatMessage::User { content: map_user_content(content)? })
            }
            ChatMessage::Assistant { content, reasoning, tool_calls, .. } => {
                let openai_tool_calls: Vec<_> = tool_calls
                    .iter()
                    .map(|call| ToolCall::Function {
                        id: call.id.clone(),
                        function: FunctionCall { name: call.name.clone(), arguments: call.arguments.clone() },
                    })
                    .collect();

                let has_tool_calls = !openai_tool_calls.is_empty();
                let tool_calls = has_tool_calls.then_some(openai_tool_calls);

                let reasoning_content = if reasoning.summary_text.is_some() {
                    reasoning.summary_text.clone()
                } else if has_tool_calls && tool_call_reasoning_placeholder {
                    Some(".".to_string())
                } else {
                    None
                };

                Some(CompatibleChatMessage::Assistant { content: content.clone(), reasoning_content, tool_calls })
            }
            ChatMessage::ToolCallResult(result) => {
                let (tool_call_id, content, _) = tool_result_parts(result);
                Some(CompatibleChatMessage::Tool {
                    content: content.to_string(),
                    tool_call_id: tool_call_id.to_string(),
                })
            }
            ChatMessage::Summary { .. } | ChatMessage::Error { .. } => {
                message.as_user_note().map(|note| CompatibleChatMessage::User { content: UserContent::Text(note) })
            }
        };

        if let Some(msg) = mapped {
            result.push(msg);
        }
    }

    Ok(result)
}

fn map_user_content(parts: &[ContentBlock]) -> crate::Result<UserContent> {
    let has_non_text = parts.iter().any(|p| !matches!(p, ContentBlock::Text { .. }));

    if !has_non_text {
        return Ok(UserContent::Text(ContentBlock::join_text(parts)));
    }

    let mut items = Vec::with_capacity(parts.len());
    for p in parts {
        match p {
            ContentBlock::Text { text } => items.push(UserContentPart::Text { text: text.clone() }),
            ContentBlock::Image { .. } => {
                items.push(UserContentPart::ImageUrl { image_url: ImageUrlContent { url: p.as_data_uri().unwrap() } });
            }
            ContentBlock::Audio { .. } => {
                return Err(crate::LlmError::UnsupportedContent("This provider does not support audio input".into()));
            }
        }
    }

    Ok(UserContent::Parts(items))
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ChatCompletionStreamChoice {
    pub delta: ChatCompletionStreamResponseDelta,
    pub finish_reason: Option<FinishReason>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct ChatCompletionStreamResponseDelta {
    pub content: Option<String>,
    pub reasoning_content: Option<String>,
    pub tool_calls: Option<Vec<ToolCallDelta>>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ToolCallDelta {
    pub index: i32,
    pub id: Option<String>,
    pub function: Option<FunctionCallDelta>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct FunctionCallDelta {
    pub name: Option<String>,
    pub arguments: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::openai::chat::{ChatRequestPolicy, build_chat_request};
    use crate::types::IsoString;
    use crate::{AssistantReasoning, Context, MessageId, ModelSettings, ToolCallRequest, ToolDefinition};

    fn assistant_with_tool_call(reasoning_content: Option<&str>) -> ChatMessage {
        ChatMessage::Assistant {
            message_id: MessageId::new(),
            content: String::new(),
            reasoning: AssistantReasoning {
                summary_text: reasoning_content.map(ToString::to_string),
                encrypted_content: None,
            },
            timestamp: IsoString::now(),
            tool_calls: vec![ToolCallRequest {
                id: "call_1".to_string(),
                name: "test__tool".to_string(),
                arguments: "{\"path\":\"src/main.rs\"}".to_string(),
            }],
        }
    }

    fn context_with_assistant_message(message: ChatMessage) -> crate::Context {
        crate::Context::new(
            vec![ChatMessage::user("run a tool"), message],
            vec![ToolDefinition::new("test__tool", "test", serde_json::json!({ "type": "object" }))],
        )
    }

    #[test]
    fn test_build_request_includes_reasoning_content_on_assistant_tool_message() {
        let context = context_with_assistant_message(assistant_with_tool_call(Some("trace chunk")));
        let request = build_chat_request("test-model", &context, &ChatRequestPolicy::DEFAULT).unwrap();

        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["messages"][1]["role"], "assistant");
        assert_eq!(json["messages"][1]["reasoning_content"], "trace chunk");
    }

    #[test]
    fn test_build_request_maps_model_settings_and_omits_when_unset() {
        let user = || ChatMessage::user("hello");

        let mut context = Context::new(vec![user()], vec![]);
        context.set_model_settings(ModelSettings { temperature: Some(0.0), top_p: Some(0.5), max_tokens: Some(64) });
        let json =
            serde_json::to_value(build_chat_request("test-model", &context, &ChatRequestPolicy::DEFAULT).unwrap())
                .unwrap();
        assert_eq!(json["temperature"], 0.0);
        assert_eq!(json["top_p"], 0.5);
        assert_eq!(json["max_tokens"], 64);

        let unset = Context::new(vec![user()], vec![]);
        let json = serde_json::to_value(build_chat_request("test-model", &unset, &ChatRequestPolicy::DEFAULT).unwrap())
            .unwrap();
        assert!(json.get("temperature").is_none());
        assert!(json.get("top_p").is_none());
        assert!(json.get("max_tokens").is_none());
    }

    #[test]
    fn test_build_request_includes_stream_options_with_usage() {
        let context = crate::Context::new(vec![ChatMessage::user("hello")], vec![]);
        let request = build_chat_request("test-model", &context, &ChatRequestPolicy::DEFAULT).unwrap();

        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["stream_options"]["include_usage"], true);
    }

    #[test]
    fn test_build_request_sends_empty_reasoning_content_on_tool_call_when_none() {
        let context = context_with_assistant_message(assistant_with_tool_call(None));
        let request = build_chat_request("test-model", &context, &ChatRequestPolicy::DEFAULT).unwrap();

        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["messages"][1]["role"], "assistant");
        assert_eq!(json["messages"][1]["reasoning_content"], ".");

        let policy = ChatRequestPolicy { tool_call_reasoning_placeholder: false, ..ChatRequestPolicy::DEFAULT };
        let json = serde_json::to_value(build_chat_request("test-model", &context, &policy).unwrap()).unwrap();
        assert!(json["messages"][1].get("reasoning_content").is_none());
    }

    #[test]
    fn test_user_message_text_only_serializes_as_string() {
        let content = map_user_content(&[ContentBlock::text("Hello")]).unwrap();
        let json = serde_json::to_value(&content).unwrap();
        assert_eq!(json, "Hello");
    }

    #[test]
    fn test_user_message_with_image_serializes_as_array() {
        let content = map_user_content(&[
            ContentBlock::text("Look:"),
            ContentBlock::Image { data: "aW1n".to_string(), mime_type: "image/png".to_string() },
        ])
        .unwrap();
        let json = serde_json::to_value(&content).unwrap();
        let parts = json.as_array().expect("Expected array");
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["type"], "text");
        assert_eq!(parts[0]["text"], "Look:");
        assert_eq!(parts[1]["type"], "image_url");
        assert!(parts[1]["image_url"]["url"].as_str().unwrap().starts_with("data:image/png;base64,"));
    }

    #[test]
    fn test_user_message_audio_only_errors() {
        let result = map_user_content(&[ContentBlock::Audio {
            data: "YXVkaW8=".to_string(),
            mime_type: "audio/wav".to_string(),
        }]);
        assert!(matches!(result, Err(crate::LlmError::UnsupportedContent(_))));
    }

    #[test]
    fn test_user_message_audio_with_text_errors() {
        let result = map_user_content(&[
            ContentBlock::text("Listen:"),
            ContentBlock::Audio { data: "YXVkaW8=".to_string(), mime_type: "audio/wav".to_string() },
        ]);
        assert!(matches!(result, Err(crate::LlmError::UnsupportedContent(_))));
    }
}
