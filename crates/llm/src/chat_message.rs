use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::catalog::LlmModel;
use crate::types::IsoString;

use super::{AssistantReply, MessageId, ToolCallError, ToolCallResult};

#[doc = include_str!("docs/content_block.md")]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ContentBlock {
    Text { text: String },
    Image { data: String, mime_type: String },
    Audio { data: String, mime_type: String },
}

impl ContentBlock {
    pub fn text(s: impl Into<String>) -> Self {
        ContentBlock::Text { text: s.into() }
    }

    pub fn estimated_bytes(&self) -> usize {
        match self {
            ContentBlock::Text { text } => text.len(),
            ContentBlock::Image { data, .. } | ContentBlock::Audio { data, .. } => data.len(),
        }
    }

    pub fn is_image(&self) -> bool {
        matches!(self, ContentBlock::Image { .. })
    }

    pub fn first_text(parts: &[ContentBlock]) -> Option<&str> {
        parts.iter().find_map(|part| match part {
            ContentBlock::Text { text } => {
                let trimmed = text.trim();
                (!trimmed.is_empty()).then_some(trimmed)
            }
            _ => None,
        })
    }

    /// Joins all text blocks with newlines, ignoring non-text content.
    pub fn join_text(parts: &[ContentBlock]) -> String {
        parts
            .iter()
            .filter_map(|p| match p {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Returns a `data:{mime};base64,{data}` URI for image/audio blocks, `None` for text.
    pub fn as_data_uri(&self) -> Option<String> {
        match self {
            ContentBlock::Image { data, mime_type } | ContentBlock::Audio { data, mime_type } => {
                Some(format!("data:{mime_type};base64,{data}"))
            }
            ContentBlock::Text { .. } => None,
        }
    }
}

#[doc = include_str!("docs/chat_message.md")]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ChatMessage {
    System {
        content: String,
        timestamp: IsoString,
    },
    User {
        message_id: MessageId,
        content: Vec<ContentBlock>,
        timestamp: IsoString,
    },
    Assistant {
        message_id: MessageId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<LlmModel>,
        content: AssistantReply,
        timestamp: IsoString,
    },
    ToolCallResult(Result<ToolCallResult, ToolCallError>),
    Error {
        message_id: MessageId,
        message: String,
        timestamp: IsoString,
    },
    /// A compacted summary of previous conversation history.
    /// This replaces multiple messages with a structured summary to reduce context usage.
    Summary {
        message_id: MessageId,
        content: String,
        timestamp: IsoString,
        /// Number of messages that were compacted into this summary
        messages_compacted: usize,
    },
}

impl ChatMessage {
    /// A user message carrying a single block of text, stamped now.
    pub fn user(text: impl Into<String>) -> Self {
        Self::user_with_id(MessageId::new(), vec![ContentBlock::text(text)])
    }

    /// A system prompt, stamped now.
    pub fn system(content: impl Into<String>) -> Self {
        ChatMessage::System { content: content.into(), timestamp: IsoString::now() }
    }

    pub fn user_with_id(message_id: MessageId, content: Vec<ContentBlock>) -> Self {
        Self::User { message_id, content, timestamp: IsoString::now() }
    }

    pub fn assistant_reply(
        message_id: MessageId,
        model: Option<LlmModel>,
        mut content: AssistantReply,
        results: &[Result<ToolCallResult, ToolCallError>],
    ) -> Self {
        content.pair_tool_calls(results);
        Self::Assistant { message_id, model, content, timestamp: IsoString::now() }
    }

    /// Identity of every message that can be addressed by clients; system prompts have none.
    pub fn message_id(&self) -> Option<MessageId> {
        match self {
            Self::User { message_id, .. }
            | Self::Assistant { message_id, .. }
            | Self::Error { message_id, .. }
            | Self::Summary { message_id, .. } => Some(message_id.clone()),
            Self::ToolCallResult(Ok(result)) => Some(MessageId::tool_result(&result.id)),
            Self::ToolCallResult(Err(error)) => Some(MessageId::tool_result(&error.id)),
            Self::System { .. } => None,
        }
    }

    /// Returns true if this message is a tool call result
    pub fn is_tool_result(&self) -> bool {
        matches!(self, ChatMessage::ToolCallResult(_))
    }

    /// Returns true if this message is a system prompt
    pub fn is_system(&self) -> bool {
        matches!(self, ChatMessage::System { .. })
    }

    /// Returns true if this message is a compacted summary
    pub fn is_summary(&self) -> bool {
        matches!(self, ChatMessage::Summary { .. })
    }

    /// Rough byte-size estimate of the message content for pre-flight context checks.
    /// Not meant to be exact — just close enough to detect overflow before calling the LLM.
    pub fn estimated_bytes(&self) -> usize {
        match self {
            ChatMessage::System { content, .. }
            | ChatMessage::Error { message: content, .. }
            | ChatMessage::Summary { content, .. } => content.len(),
            ChatMessage::User { content, .. } => content.iter().map(ContentBlock::estimated_bytes).sum(),
            ChatMessage::Assistant { content, .. } => content.estimated_bytes(),
            ChatMessage::ToolCallResult(Ok(result)) => result.name.len() + result.arguments.len() + result.result.len(),
            ChatMessage::ToolCallResult(Err(error)) => {
                error.name.len() + error.arguments.as_ref().map_or(0, String::len) + error.error.len()
            }
        }
    }

    /// Returns the timestamp of this message, if it has one
    pub fn timestamp(&self) -> Option<&IsoString> {
        match self {
            ChatMessage::System { timestamp, .. }
            | ChatMessage::User { timestamp, .. }
            | ChatMessage::Assistant { timestamp, .. }
            | ChatMessage::Error { timestamp, .. }
            | ChatMessage::Summary { timestamp, .. } => Some(timestamp),
            ChatMessage::ToolCallResult(_) => None,
        }
    }

    pub(crate) fn as_user_note(&self) -> Option<String> {
        match self {
            ChatMessage::Summary { content, .. } => Some(format!("[Previous conversation handoff]\n\n{content}")),
            ChatMessage::Error { message, .. } => Some(format!("Error: {message}")),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assistant_reply_pairs_every_tool_call_with_a_result() {
        let model: LlmModel = "anthropic:claude-opus-4-6".parse().unwrap();
        let content = crate::testing::assistant_message()
            .tool_call("call_1", "read", "{}")
            .tool_call("call_cancelled", "read", "{}")
            .build_reply();
        let result = ToolCallResult {
            id: "call_1".to_string(),
            name: "read".to_string(),
            arguments: "{}".to_string(),
            result: "ok".to_string(),
        };
        let error = ToolCallError {
            id: "call_retried".to_string(),
            name: "bash".to_string(),
            arguments: None,
            error: "failed".to_string(),
        };

        let reply =
            ChatMessage::assistant_reply(MessageId::new(), Some(model.clone()), content, &[Ok(result), Err(error)]);

        let ChatMessage::Assistant { model: author, content, .. } = reply else { panic!("expected assistant message") };
        let ids: Vec<_> = content.tool_calls().map(|call| call.id.as_str()).collect();
        assert_eq!(ids, ["call_1", "call_retried"]);
        assert_eq!(author, Some(model));
    }

    #[test]
    fn first_text_returns_first_non_empty_text_block() {
        let parts = vec![
            ContentBlock::Image { data: "a".to_string(), mime_type: "image/png".to_string() },
            ContentBlock::text(" "),
            ContentBlock::text("hello"),
        ];

        assert_eq!(ContentBlock::first_text(&parts), Some("hello"));
    }

    #[test]
    fn estimated_bytes_counts_reasoning_in_the_form_it_is_sent_back() {
        let summary = "s".repeat(10);
        let encrypted = "e".repeat(100);
        let assistant = crate::testing::assistant_message().text("hi");

        assert_eq!(assistant.clone().reasoning(&summary).build().estimated_bytes(), 12);
        assert_eq!(assistant.reasoning(&summary).encrypted_reasoning("r_1", &encrypted).build().estimated_bytes(), 102);
    }

    #[test]
    fn assistant_messages_roundtrip_with_their_author() {
        let message = crate::testing::assistant_message()
            .model("anthropic:claude-opus-4-6")
            .reasoning("plan")
            .encrypted_reasoning("r_1", "blob")
            .text("answer")
            .build();

        let json = serde_json::to_value(&message).unwrap();

        assert_eq!(json["model"], "anthropic:claude-opus-4-6");
        assert_eq!(serde_json::from_value::<ChatMessage>(json).unwrap(), message);
    }
}
