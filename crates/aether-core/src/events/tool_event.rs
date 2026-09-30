use super::SubAgentProgressPayload;
use llm::types::IsoString;
use llm::{ChatMessage, ContentBlock, MessageId, ToolCallError, ToolCallRequest, ToolCallResult, ToolDefinition};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use utils::display_meta::ToolResultMeta;

/// Tool call lifecycle events.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolEvent {
    InputStarted {
        id: String,
        name: String,
    },
    InputDelta {
        id: String,
        chunk: String,
    },
    Call {
        request: ToolCallRequest,
    },
    Progress {
        request: ToolCallRequest,
        progress: f64,
        total: Option<f64>,
        message: Option<String>,
    },
    SubAgentProgress {
        request: ToolCallRequest,
        payload: Box<SubAgentProgressPayload>,
    },
    DisplayUpdate {
        request: ToolCallRequest,
        meta: ToolResultMeta,
    },
    TaskCreated {
        request: ToolCallRequest,
        task_id: String,
        status_message: Option<String>,
    },
    TaskStatus {
        request: ToolCallRequest,
        task_id: String,
        status: String,
        status_message: Option<String>,
    },
    TaskCompleted {
        request: ToolCallRequest,
        task_id: String,
        result: ToolCallResult,
        result_meta: Option<ToolResultMeta>,
    },
    TaskFailed {
        request: ToolCallRequest,
        task_id: String,
        error: ToolCallError,
    },
    TaskCancelled {
        request: ToolCallRequest,
        task_id: String,
    },
    Result {
        result: ToolCallResult,
        result_meta: Option<ToolResultMeta>,
    },
    Error {
        error: ToolCallError,
    },
    DefinitionsUpdated {
        tools: Vec<ToolDefinition>,
    },
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
        let (status, body) = self.status_body();
        task_result_message(&self.request, &self.task_id, status, body)
    }

    pub fn content_blocks(&self) -> Vec<ContentBlock> {
        let (status, body) = self.status_body();
        task_result_content(&self.request, &self.task_id, status, body)
    }

    fn status_body(&self) -> (&str, &str) {
        match &self.state {
            TaskOutcomeState::Completed { result, .. } => ("completed", result.result.as_str()),
            TaskOutcomeState::Failed { error } => ("failed", error.error.as_str()),
            TaskOutcomeState::Cancelled => ("cancelled", TASK_CANCELLED_BODY),
        }
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
    ChatMessage::User {
        message_id: MessageId::task_result(task_id),
        content: task_result_content(request, task_id, status, body),
        timestamp: IsoString::now(),
    }
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
