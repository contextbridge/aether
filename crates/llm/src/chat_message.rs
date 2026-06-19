use serde::{Deserialize, Serialize};

use crate::catalog::LlmModel;
use crate::types::IsoString;

use super::{ToolCallError, ToolCallRequest, ToolCallResult};

#[doc = include_str!("docs/content_block.md")]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

/// Opaque encrypted reasoning content from an LLM response.
///
/// This is model-specific: encrypted content from one model cannot be replayed
/// to a different model. Use [`Context::filter_encrypted_reasoning`](crate::Context::filter_encrypted_reasoning)
/// to strip content that doesn't match the target model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EncryptedReasoningContent {
    pub id: String,
    #[serde(serialize_with = "serialize_llm_model", deserialize_with = "deserialize_llm_model")]
    pub model: LlmModel,
    pub content: String,
}

/// Reasoning metadata from an assistant response.
///
/// Contains an optional human-readable summary and optional encrypted content
/// that can be replayed to the same model in future turns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AssistantReasoning {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_text: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encrypted_content: Option<EncryptedReasoningContent>,
}

impl AssistantReasoning {
    pub fn from_parts(summary_text: String, encrypted: Option<EncryptedReasoningContent>) -> Self {
        Self { summary_text: (!summary_text.is_empty()).then_some(summary_text), encrypted_content: encrypted }
    }

    pub fn is_empty(&self) -> bool {
        self.summary_text.is_none() && self.encrypted_content.is_none()
    }
}

#[doc = include_str!("docs/chat_message.md")]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ChatMessage {
    System {
        content: String,
        timestamp: IsoString,
    },
    User {
        content: Vec<ContentBlock>,
        timestamp: IsoString,
    },
    Assistant {
        content: String,
        #[serde(default)]
        reasoning: AssistantReasoning,
        timestamp: IsoString,
        tool_calls: Vec<ToolCallRequest>,
    },
    ToolCallResult(Result<ToolCallResult, ToolCallError>),
    Error {
        message: String,
        timestamp: IsoString,
    },
    /// A compacted summary of previous conversation history.
    /// This replaces multiple messages with a structured summary to reduce context usage.
    Summary {
        content: String,
        timestamp: IsoString,
        /// Number of messages that were compacted into this summary
        messages_compacted: usize,
    },
}

impl ChatMessage {
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
            ChatMessage::Assistant { content, reasoning, tool_calls, .. } => {
                content.len()
                    + reasoning.summary_text.as_ref().map_or(0, String::len)
                    + reasoning.encrypted_content.as_ref().map_or(0, |ec| ec.content.len())
                    + tool_calls.iter().map(|tc| tc.name.len() + tc.arguments.len()).sum::<usize>()
            }
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
}

fn serialize_llm_model<S: serde::Serializer>(model: &LlmModel, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&model.to_string())
}

fn deserialize_llm_model<'de, D: serde::Deserializer<'de>>(d: D) -> Result<LlmModel, D::Error> {
    let s = String::deserialize(d)?;
    s.parse::<LlmModel>().map_err(serde::de::Error::custom)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_model() -> LlmModel {
        "anthropic:claude-opus-4-6".parse().unwrap()
    }

    #[test]
    fn assistant_reasoning_is_empty_when_default() {
        let r = AssistantReasoning::default();
        assert!(r.is_empty());
    }

    #[test]
    fn assistant_reasoning_not_empty_with_summary() {
        let r = AssistantReasoning::from_parts("thinking".to_string(), None);
        assert!(!r.is_empty());
    }

    #[test]
    fn assistant_reasoning_not_empty_with_encrypted() {
        let r = AssistantReasoning {
            summary_text: None,
            encrypted_content: Some(EncryptedReasoningContent {
                id: "r_test".to_string(),
                model: make_model(),
                content: "blob".to_string(),
            }),
        };
        assert!(!r.is_empty());
    }

    #[test]
    fn from_parts_empty_summary_is_none() {
        let r = AssistantReasoning::from_parts(String::new(), None);
        assert!(r.summary_text.is_none());
        assert!(r.is_empty());
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
    fn encrypted_reasoning_content_serde_roundtrip() {
        let model = make_model();
        let ec = EncryptedReasoningContent {
            id: "r_test".to_string(),
            model: model.clone(),
            content: "encrypted-data".to_string(),
        };
        let json = serde_json::to_string(&ec).unwrap();
        let parsed: EncryptedReasoningContent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.model, model);
        assert_eq!(parsed.content, "encrypted-data");
    }

    #[test]
    fn assistant_reasoning_serde_roundtrip() {
        let model = make_model();
        let r = AssistantReasoning {
            summary_text: Some("thought".to_string()),
            encrypted_content: Some(EncryptedReasoningContent {
                id: "r_test".to_string(),
                model,
                content: "blob".to_string(),
            }),
        };
        let json = serde_json::to_string(&r).unwrap();
        let parsed: AssistantReasoning = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, r);
    }

    #[test]
    fn assistant_reasoning_serde_empty_roundtrip() {
        let r = AssistantReasoning::default();
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(json, "{}");
        let parsed: AssistantReasoning = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, r);
    }

    #[test]
    fn chat_message_assistant_serde_roundtrip_with_reasoning() {
        let model = make_model();
        let msg = ChatMessage::Assistant {
            content: "response".to_string(),
            reasoning: AssistantReasoning {
                summary_text: Some("plan".to_string()),
                encrypted_content: Some(EncryptedReasoningContent {
                    id: "r_test".to_string(),
                    model,
                    content: "enc".to_string(),
                }),
            },
            timestamp: IsoString::now(),
            tool_calls: vec![],
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: ChatMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn estimated_bytes_includes_encrypted_content() {
        let model = make_model();
        let msg_with = ChatMessage::Assistant {
            content: "hi".to_string(),
            reasoning: AssistantReasoning {
                summary_text: Some("think".to_string()),
                encrypted_content: Some(EncryptedReasoningContent {
                    id: "r_test".to_string(),
                    model,
                    content: "x".repeat(100),
                }),
            },
            timestamp: IsoString::now(),
            tool_calls: vec![],
        };
        let msg_without = ChatMessage::Assistant {
            content: "hi".to_string(),
            reasoning: AssistantReasoning { summary_text: Some("think".to_string()), encrypted_content: None },
            timestamp: IsoString::now(),
            tool_calls: vec![],
        };
        assert!(msg_with.estimated_bytes() > msg_without.estimated_bytes());
    }
}
