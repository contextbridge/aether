use acp_utils::notifications::{
    SubAgentEvent, SubAgentToolCallUpdate, SubAgentToolError, SubAgentToolRequest, SubAgentToolResult,
};
use llm::{ToolCallError, ToolCallRequest, ToolCallResult};
use mcp_utils::display_meta::ToolResultMeta;
use serde::{Deserialize, Serialize};

/// Message from the agent to the user.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentMessage {
    Text {
        message_id: String,
        chunk: String,
        is_complete: bool,
        model_name: String,
    },

    Thought {
        message_id: String,
        chunk: String,
        is_complete: bool,
        model_name: String,
    },

    ToolCall {
        request: ToolCallRequest,
        model_name: String,
    },

    ToolCallUpdate {
        tool_call_id: String,
        chunk: String,
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
        result_meta: Option<ToolResultMeta>,
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

    /// Context compaction has been triggered.
    ContextCompactionStarted {
        message_count: usize,
    },

    /// Context was compacted to reduce token usage.
    ContextCompactionResult {
        summary: String,
        messages_removed: usize,
    },

    /// Context usage update for UI display.
    #[serde(rename = "context_usage")]
    ContextUsageUpdate {
        /// Current usage ratio (0.0 - 1.0), if context window is known.
        usage_ratio: Option<f64>,
        /// Maximum context limit, if known.
        context_limit: Option<u32>,
        /// Input tokens on the most recent API call (the current context size).
        input_tokens: u32,
        /// Output tokens on the most recent API call.
        output_tokens: u32,
        /// Prompt tokens served from cache on the most recent API call.
        cache_read_tokens: Option<u32>,
        /// Prompt tokens written to cache on the most recent API call.
        cache_creation_tokens: Option<u32>,
        /// Reasoning tokens spent on the most recent API call.
        reasoning_tokens: Option<u32>,
        /// Cumulative input tokens since the agent started.
        total_input_tokens: u64,
        /// Cumulative output tokens since the agent started.
        total_output_tokens: u64,
        /// Cumulative cache-read tokens since the agent started.
        total_cache_read_tokens: u64,
        /// Cumulative cache-creation tokens since the agent started.
        total_cache_creation_tokens: u64,
        /// Cumulative reasoning tokens since the agent started.
        total_reasoning_tokens: u64,
    },

    /// Agent is auto-continuing because LLM stopped with a resumable stop reason.
    AutoContinue {
        /// Current attempt number (1-indexed).
        attempt: u32,
        /// Maximum allowed attempts.
        max_attempts: u32,
    },

    /// Agent is retrying after a transient LLM provider failure
    Retrying {
        /// Current retry attempt number (1-indexed).
        attempt: u32,
        /// Maximum allowed attempts.
        max_attempts: u32,
        /// Backoff delay in milliseconds before the retry fires.
        delay_ms: u64,
        /// The error that triggered the retry, formatted for display.
        error: String,
    },

    /// The model was successfully switched.
    ModelSwitched {
        previous: String,
        new: String,
    },

    /// The agent context was cleared and reset to its blank state.
    ContextCleared,

    Done,
}

impl From<&AgentMessage> for SubAgentEvent {
    fn from(msg: &AgentMessage) -> Self {
        match msg {
            AgentMessage::ToolCall { request, .. } => SubAgentEvent::ToolCall {
                request: SubAgentToolRequest {
                    id: request.id.clone(),
                    name: request.name.clone(),
                    arguments: request.arguments.clone(),
                },
            },
            AgentMessage::ToolCallUpdate { tool_call_id, chunk, .. } => SubAgentEvent::ToolCallUpdate {
                update: SubAgentToolCallUpdate { id: tool_call_id.clone(), chunk: chunk.clone() },
            },
            AgentMessage::ToolResult { result, result_meta, .. } => SubAgentEvent::ToolResult {
                result: SubAgentToolResult {
                    id: result.id.clone(),
                    name: result.name.clone(),
                    result_meta: result_meta.clone(),
                },
            },
            AgentMessage::ToolError { error, .. } => {
                SubAgentEvent::ToolError { error: SubAgentToolError { id: error.id.clone(), name: error.name.clone() } }
            }
            AgentMessage::Done => SubAgentEvent::Done,
            _ => SubAgentEvent::Other,
        }
    }
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

    pub fn thought(message_id: &str, chunk: &str, is_complete: bool, model_name: &str) -> Self {
        AgentMessage::Thought {
            message_id: message_id.to_string(),
            chunk: chunk.to_string(),
            is_complete,
            model_name: model_name.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AgentMessage;
    use acp_utils::notifications::SubAgentEvent;
    use llm::ToolCallResult;
    use mcp_utils::display_meta::ToolDisplayMeta;

    #[test]
    fn test_model_switched_serde_roundtrip() {
        let msg = AgentMessage::ModelSwitched {
            previous: "anthropic:claude-3.5-sonnet".to_string(),
            new: "ollama:llama3.2".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: AgentMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn test_thought_serde_roundtrip() {
        let msg = AgentMessage::Thought {
            message_id: "msg_1".to_string(),
            chunk: "thinking".to_string(),
            is_complete: false,
            model_name: "test-model".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: AgentMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn test_thought_complete_serde_roundtrip() {
        let msg = AgentMessage::Thought {
            message_id: "msg_1".to_string(),
            chunk: "full reasoning".to_string(),
            is_complete: true,
            model_name: "test-model".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: AgentMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn test_tool_result_serializes_result_meta() {
        let msg = AgentMessage::ToolResult {
            result: ToolCallResult {
                id: "call_1".to_string(),
                name: "coding__read_file".to_string(),
                arguments: r#"{"filePath":"Cargo.toml"}"#.to_string(),
                result: "ok".to_string(),
            },
            result_meta: Some(ToolDisplayMeta::new("Read file", "Cargo.toml, 156 lines").into()),
            model_name: "test-model".to_string(),
        };

        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "tool_result");
        assert_eq!(json["result_meta"]["display"]["title"], "Read file");
        assert_eq!(json["result_meta"]["display"]["value"], "Cargo.toml, 156 lines");

        let parsed: AgentMessage = serde_json::from_value(json).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn test_sub_agent_tool_result_includes_display_fields() {
        let msg = AgentMessage::ToolResult {
            result: ToolCallResult {
                id: "call_1".to_string(),
                name: "coding__read_file".to_string(),
                arguments: r#"{"filePath":"Cargo.toml"}"#.to_string(),
                result: "ok".to_string(),
            },
            result_meta: Some(ToolDisplayMeta::new("Read file", "Cargo.toml, 156 lines").into()),
            model_name: "test-model".to_string(),
        };

        let event: SubAgentEvent = (&msg).into();
        match event {
            SubAgentEvent::ToolResult { result } => {
                assert_eq!(result.id, "call_1");
                assert_eq!(result.name, "coding__read_file");
                let result_meta = result.result_meta.expect("result_meta should be present");
                assert_eq!(result_meta.display.title, "Read file");
                assert_eq!(result_meta.display.value, "Cargo.toml, 156 lines");
            }
            other => panic!("Expected ToolResult, got {other:?}"),
        }
    }

    #[test]
    fn test_sub_agent_tool_call_update_includes_updated_fields() {
        let msg = AgentMessage::ToolCallUpdate {
            tool_call_id: "call_1".to_string(),
            chunk: r#"{"filePath":"Cargo.toml"}"#.to_string(),
            model_name: "test-model".to_string(),
        };

        let event: SubAgentEvent = (&msg).into();
        match event {
            SubAgentEvent::ToolCallUpdate { update } => {
                assert_eq!(update.id, "call_1");
                assert_eq!(update.chunk, r#"{"filePath":"Cargo.toml"}"#);
            }
            other => panic!("Expected ToolCallUpdate, got {other:?}"),
        }
    }

    #[test]
    fn test_done_serializes_as_object() {
        let json = serde_json::to_value(&AgentMessage::Done).unwrap();
        assert_eq!(json["type"], "done");
        assert_eq!(json.as_object().unwrap().len(), 1);

        let parsed: AgentMessage = serde_json::from_value(json).unwrap();
        assert_eq!(parsed, AgentMessage::Done);
    }

    #[test]
    fn test_tool_result_roundtrip_with_type_tag() {
        let msg = AgentMessage::ToolResult {
            result: ToolCallResult {
                id: "call_1".to_string(),
                name: "coding__read_file".to_string(),
                arguments: "{}".to_string(),
                result: "ok".to_string(),
            },
            result_meta: None,
            model_name: "test".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"tool_result""#), "missing type tag: {json}");

        let parsed: AgentMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn test_context_usage_serializes_with_type_tag() {
        let msg = AgentMessage::ContextUsageUpdate {
            usage_ratio: Some(0.5),
            context_limit: Some(200_000),
            input_tokens: 1000,
            output_tokens: 200,
            cache_read_tokens: None,
            cache_creation_tokens: None,
            reasoning_tokens: None,
            total_input_tokens: 3000,
            total_output_tokens: 600,
            total_cache_read_tokens: 0,
            total_cache_creation_tokens: 0,
            total_reasoning_tokens: 0,
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "context_usage");

        let parsed: AgentMessage = serde_json::from_value(json).unwrap();
        assert_eq!(parsed, msg);
    }
}
