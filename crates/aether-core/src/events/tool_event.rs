use llm::{ToolCallError, ToolCallRequest, ToolCallResult, ToolDefinition};
use mcp_utils::display_meta::ToolResultMeta;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Tool call lifecycle events.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolEvent {
    /// The LLM requested a tool call; arguments may still be streaming.
    Call { request: ToolCallRequest },
    /// A chunk of streamed tool call arguments.
    CallUpdate { tool_call_id: String, chunk: String },
    /// The tool began executing.
    ExecutionStarted { tool_id: String, tool_name: String },
    /// Progress reported by an executing tool.
    Progress { request: ToolCallRequest, progress: f64, total: Option<f64>, message: Option<String> },
    /// The tool completed successfully.
    Result { result: ToolCallResult, result_meta: Option<ToolResultMeta> },
    /// The tool failed.
    Error { error: ToolCallError },
    /// The set of available tool definitions changed.
    DefinitionsUpdated { tools: Vec<ToolDefinition> },
}
