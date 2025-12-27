use serde::{Deserialize, Serialize};

use crate::llm::{ToolCallError, ToolCallRequest, ToolCallResult};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AgentMessage {
    Text {
        message_id: String,
        chunk: String,
        is_complete: bool,
        model_name: String,
    },

    ToolCall {
        request: ToolCallRequest,
        model_name: String,
    },

    ToolProgress {
        request: ToolCallRequest,
        progress: f64,
        total: Option<f64>,
        message: Option<String>,
    },

    ToolResult {
        result: ToolCallResult,
        model_name: String,
    },

    ToolError {
        error: ToolCallError,
        model_name: String,
    },

    Error {
        message: String,
    },

    Cancelled {
        message: String,
    },

    /// Context was compacted to reduce token usage
    ContextCompacted {
        /// The summary that replaced the compacted messages
        summary: String,
        /// Number of messages that were removed
        messages_removed: usize,
    },

    Done,
}

#[derive(Debug, Clone)]
pub enum UserMessage {
    Text { content: String },
    Cancel,
}

impl AgentMessage {
    pub fn text(message_id: &str, chunk: &str, is_complete: bool, model_name: &str) -> Self {
        AgentMessage::Text {
            message_id: message_id.to_string(),
            chunk: chunk.to_string(),
            is_complete,
            model_name: model_name.to_string(),
        }
    }
}

impl UserMessage {
    pub fn text(content: &str) -> Self {
        UserMessage::Text {
            content: content.to_string(),
        }
    }
}

impl From<&str> for UserMessage {
    fn from(value: &str) -> Self {
        UserMessage::Text {
            content: value.to_string(),
        }
    }
}
