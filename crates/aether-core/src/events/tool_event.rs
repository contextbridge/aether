use llm::{ToolCallError, ToolCallRequest, ToolCallResult, ToolDefinition};
use mcp_utils::display_meta::ToolResultMeta;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolEvent {
    Call { request: ToolCallRequest, model_name: String },
    CallUpdate { tool_call_id: String, chunk: String, model_name: String },
    ExecutionStarted { tool_id: String, tool_name: String },
    Progress { request: ToolCallRequest, progress: f64, total: Option<f64>, message: Option<String> },
    Result { result: ToolCallResult, result_meta: Option<ToolResultMeta>, model_name: String },
    Error { error: ToolCallError, model_name: String },
    DefinitionsUpdated { tools: Vec<ToolDefinition> },
}
