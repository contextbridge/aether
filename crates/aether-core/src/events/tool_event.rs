use llm::types::IsoString;
use llm::{ChatMessage, ContentBlock, ToolCallError, ToolCallRequest, ToolCallResult, ToolDefinition};
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

impl ToolEvent {
    /// The context message describing a terminal background-task event, or
    /// `None` for every other event.
    pub fn task_context_message(&self) -> Option<ChatMessage> {
        let (request, task_id, status, body) = match self {
            Self::TaskCompleted { request, task_id, result, .. } => {
                (request, task_id, "completed", result.result.as_str())
            }
            Self::TaskFailed { request, task_id, error } => (request, task_id, "failed", error.error.as_str()),
            Self::TaskCancelled { request, task_id } => (request, task_id, "cancelled", TASK_CANCELLED_BODY),
            _ => return None,
        };
        Some(task_result_message(request, task_id, status, body))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TaskOutcome {
    pub request: ToolCallRequest,
    pub task_id: String,
    pub state: TaskOutcomeState,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TaskOutcomeState {
    Completed { result: ToolCallResult, result_meta: Option<ToolResultMeta> },
    Failed { error: ToolCallError },
    Cancelled,
}

impl TaskOutcome {
    pub fn context_message(&self) -> ChatMessage {
        ChatMessage::User { content: self.content_blocks(), timestamp: IsoString::now() }
    }

    pub fn content_blocks(&self) -> Vec<ContentBlock> {
        let (status, body) = match &self.state {
            TaskOutcomeState::Completed { result, .. } => ("completed", result.result.as_str()),
            TaskOutcomeState::Failed { error } => ("failed", error.error.as_str()),
            TaskOutcomeState::Cancelled => ("cancelled", TASK_CANCELLED_BODY),
        };
        task_result_content(&self.request, &self.task_id, status, body)
    }
}

impl From<TaskOutcome> for ToolEvent {
    fn from(outcome: TaskOutcome) -> Self {
        let TaskOutcome { request, task_id, state } = outcome;
        match state {
            TaskOutcomeState::Completed { result, result_meta } => {
                Self::TaskCompleted { request, task_id, result, result_meta }
            }
            TaskOutcomeState::Failed { error } => Self::TaskFailed { request, task_id, error },
            TaskOutcomeState::Cancelled => Self::TaskCancelled { request, task_id },
        }
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

const TASK_CANCELLED_BODY: &str = "The background task was cancelled and will not produce a result.";

fn task_result_message(request: &ToolCallRequest, task_id: &str, status: &str, body: &str) -> ChatMessage {
    ChatMessage::User { content: task_result_content(request, task_id, status, body), timestamp: IsoString::now() }
}

fn task_result_content(request: &ToolCallRequest, task_id: &str, status: &str, body: &str) -> Vec<ContentBlock> {
    let content = format!(
        "<task-result task-id=\"{}\" tool=\"{}\" status=\"{status}\">{}</task-result>",
        escape_xml(task_id),
        escape_xml(&request.name),
        escape_xml(body),
    );
    vec![ContentBlock::text(content)]
}

fn escape_xml(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}
