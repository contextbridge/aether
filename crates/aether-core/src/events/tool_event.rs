use llm::types::IsoString;
use llm::{ChatMessage, ContentBlock, ToolCallError, ToolCallRequest, ToolCallResult, ToolDefinition};
use mcp_utils::display_meta::ToolResultMeta;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NonTerminalTaskEvent;

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
    /// A background task was created by a tool call response.
    TaskCreated { request: ToolCallRequest, task_id: String, status_message: Option<String> },
    /// The background task reported a new status.
    TaskStatus { request: ToolCallRequest, task_id: String, status: String, status_message: Option<String> },
    /// The background task completed successfully.
    TaskCompleted {
        request: ToolCallRequest,
        task_id: String,
        result: ToolCallResult,
        result_meta: Option<ToolResultMeta>,
    },
    /// The background task failed.
    TaskFailed { request: ToolCallRequest, task_id: String, error: ToolCallError },
    /// The background task was cancelled.
    TaskCancelled { request: ToolCallRequest, task_id: String },
    /// The tool completed successfully.
    Result { result: ToolCallResult, result_meta: Option<ToolResultMeta> },
    /// The tool failed.
    Error { error: ToolCallError },
    /// The set of available tool definitions changed.
    DefinitionsUpdated { tools: Vec<ToolDefinition> },
}

impl TryFrom<&ToolEvent> for ChatMessage {
    type Error = NonTerminalTaskEvent;

    fn try_from(event: &ToolEvent) -> Result<Self, NonTerminalTaskEvent> {
        let (request, task_id, status, body) = match event {
            ToolEvent::TaskCompleted { request, task_id, result, .. } => {
                (request, task_id, "completed", result.result.as_str())
            }
            ToolEvent::TaskFailed { request, task_id, error } => (request, task_id, "failed", error.error.as_str()),
            ToolEvent::TaskCancelled { request, task_id } => {
                (request, task_id, "cancelled", "The background task was cancelled and will not produce a result.")
            }
            _ => return Err(NonTerminalTaskEvent),
        };
        let content = format!(
            "<mcp-task-result task-id=\"{}\" tool=\"{}\" status=\"{status}\">{}</mcp-task-result>",
            escape_xml(task_id),
            escape_xml(&request.name),
            escape_xml(body),
        );
        Ok(Self::User { content: vec![ContentBlock::text(content)], timestamp: IsoString::now() })
    }
}

pub fn task_created_result(request: &ToolCallRequest, task_id: &str) -> ToolCallResult {
    ToolCallResult {
        id: request.id.clone(),
        name: request.name.clone(),
        arguments: request.arguments.clone(),
        result: format!(
            "This tool is running as a background task, id: {task_id}. The result will be automatically injected into context when it completes, you may continue working."
        ),
    }
}

fn escape_xml(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}
