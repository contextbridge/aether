use async_openai::types::chat::{
    ChatCompletionMessageToolCall, ChatCompletionMessageToolCalls,
    ChatCompletionRequestAssistantMessage, ChatCompletionRequestAssistantMessageContent,
    ChatCompletionRequestMessage, ChatCompletionRequestSystemMessage,
    ChatCompletionRequestToolMessage, ChatCompletionRequestToolMessageContent,
    ChatCompletionRequestUserMessage, ChatCompletionTool, ChatCompletionTools, FunctionCall,
    FunctionObject,
};

use crate::llm::{ChatMessage, ToolDefinition};

impl From<ChatMessage> for Option<ChatCompletionRequestMessage> {
    fn from(msg: ChatMessage) -> Self {
        match msg {
            ChatMessage::System { content, .. } => Some(ChatCompletionRequestMessage::System(
                ChatCompletionRequestSystemMessage {
                    content: content.into(),
                    name: None,
                },
            )),
            ChatMessage::User { content, .. } => Some(ChatCompletionRequestMessage::User(
                ChatCompletionRequestUserMessage {
                    content: content.into(),
                    name: None,
                },
            )),
            ChatMessage::Assistant {
                content,
                tool_calls,
                ..
            } => {
                let openai_tool_calls: Vec<_> = tool_calls
                    .into_iter()
                    .map(|call| {
                        ChatCompletionMessageToolCalls::Function(ChatCompletionMessageToolCall {
                            id: call.id,
                            function: FunctionCall {
                                name: call.name,
                                arguments: call.arguments,
                            },
                        })
                    })
                    .collect();

                let tool_calls = (!openai_tool_calls.is_empty()).then_some(openai_tool_calls);

                Some(ChatCompletionRequestMessage::Assistant(
                    ChatCompletionRequestAssistantMessage {
                        content: Some(ChatCompletionRequestAssistantMessageContent::Text(content)),
                        name: None,
                        tool_calls,
                        audio: None,
                        refusal: None,
                        #[allow(deprecated)]
                        function_call: None,
                    },
                ))
            }
            ChatMessage::ToolCallResult(result) => {
                let (content, id) = match result {
                    Ok(r) => (r.result, r.id),
                    Err(e) => (e.error, e.id),
                };
                Some(ChatCompletionRequestMessage::Tool(
                    ChatCompletionRequestToolMessage {
                        content: ChatCompletionRequestToolMessageContent::Text(content),
                        tool_call_id: id,
                    },
                ))
            }
            ChatMessage::Summary { content, .. } => {
                // Summaries from previous compaction are treated as user context
                Some(ChatCompletionRequestMessage::User(
                    ChatCompletionRequestUserMessage {
                        content: format!("[Previous conversation summary]\n\n{content}").into(),
                        name: None,
                    },
                ))
            }
            ChatMessage::Error { .. } => None,
        }
    }
}

impl From<&ChatMessage> for Option<ChatCompletionRequestMessage> {
    fn from(msg: &ChatMessage) -> Self {
        msg.clone().into()
    }
}

impl From<ToolDefinition> for ChatCompletionTools {
    fn from(tool: ToolDefinition) -> Self {
        ChatCompletionTools::Function(ChatCompletionTool {
            function: FunctionObject {
                name: tool.name,
                description: Some(tool.description),
                parameters: Some(serde_json::from_str(&tool.parameters).unwrap_or_default()),
                strict: Some(false),
            },
        })
    }
}

impl From<&ToolDefinition> for ChatCompletionTools {
    fn from(tool: &ToolDefinition) -> Self {
        tool.clone().into()
    }
}

pub fn map_messages(messages: &[ChatMessage]) -> Vec<ChatCompletionRequestMessage> {
    messages.iter().filter_map(Into::into).collect()
}

pub fn map_tools(tools: &[ToolDefinition]) -> Vec<ChatCompletionTools> {
    tools.iter().map(Into::into).collect()
}
