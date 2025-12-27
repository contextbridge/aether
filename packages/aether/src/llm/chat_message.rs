use serde::{Deserialize, Serialize};

use crate::types::IsoString;

use super::{ToolCallError, ToolCallRequest, ToolCallResult};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ChatMessage {
    System {
        content: String,
        timestamp: IsoString,
    },
    User {
        content: String,
        timestamp: IsoString,
    },
    Assistant {
        content: String,
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

    /// Returns the timestamp of this message, if it has one
    pub fn timestamp(&self) -> Option<&IsoString> {
        match self {
            ChatMessage::System { timestamp, .. } => Some(timestamp),
            ChatMessage::User { timestamp, .. } => Some(timestamp),
            ChatMessage::Assistant { timestamp, .. } => Some(timestamp),
            ChatMessage::Error { timestamp, .. } => Some(timestamp),
            ChatMessage::Summary { timestamp, .. } => Some(timestamp),
            ChatMessage::ToolCallResult(_) => None,
        }
    }
}
