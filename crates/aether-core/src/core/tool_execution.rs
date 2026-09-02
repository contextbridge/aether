use crate::events::{SubAgentProgressPayload, TaskOutcome, TaskOutcomeState, ToolEvent, task_created_result};
use crate::mcp::tool_bridge::{convert_tool_result, map_task_result_to_outcome};
use llm::{ToolCallError, ToolCallRequest, ToolCallResult};
use mcp_utils::client::{CancellationToken, ToolCallEvent};
use mcp_utils::display_meta::ToolResultMeta;
use rmcp::model::ProgressNotificationParam;
use std::collections::HashMap;

#[derive(Default)]
pub(super) struct ToolExecutions {
    executions: HashMap<String, ToolExecution>,
}

pub(super) enum ToolExecutionUpdate {
    Event(ToolEvent),
    TaskCreated { result: ToolCallResult, event: ToolEvent },
    Completed { result: Result<ToolCallResult, ToolCallError>, event: ToolEvent },
    TaskCompleted(TaskOutcome),
    TaskCancelled(TaskOutcome),
    Retired,
    Ignored,
}

pub(super) enum ToolAbortPolicy {
    CancelAll,
    PreserveBackgroundAcknowledgements,
}

struct ToolExecution {
    request: ToolCallRequest,
    cancellation_token: CancellationToken,
    phase: ToolExecutionPhase,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ToolExecutionPhase {
    Foreground,
    Background,
    Cancelling,
    Retiring,
}

impl ToolExecutions {
    pub(super) fn start(&mut self, request: ToolCallRequest) -> CancellationToken {
        let cancellation_token = CancellationToken::new();
        self.executions.insert(
            request.id.clone(),
            ToolExecution {
                request,
                cancellation_token: cancellation_token.clone(),
                phase: ToolExecutionPhase::Foreground,
            },
        );
        cancellation_token
    }

    pub(super) fn has_foreground(&self) -> bool {
        self.executions.values().any(|execution| execution.phase == ToolExecutionPhase::Foreground)
    }

    pub(super) fn is_empty(&self) -> bool {
        self.executions.is_empty()
    }

    pub(super) fn on_event(&mut self, tool_id: &str, event: ToolCallEvent) -> ToolExecutionUpdate {
        match event {
            ToolCallEvent::Progress(progress) => {
                let Some(execution) = self.executions.get(tool_id).filter(|execution| {
                    matches!(execution.phase, ToolExecutionPhase::Foreground | ToolExecutionPhase::Background)
                }) else {
                    return ToolExecutionUpdate::Ignored;
                };
                ToolExecutionUpdate::Event(progress_event(execution.request.clone(), progress))
            }
            ToolCallEvent::TaskCreated(task) => {
                let Some(execution) = self.executions.get_mut(tool_id) else {
                    return ToolExecutionUpdate::Ignored;
                };
                if execution.phase == ToolExecutionPhase::Retiring {
                    execution.cancellation_token.cancel();
                    return ToolExecutionUpdate::Ignored;
                }
                if execution.phase != ToolExecutionPhase::Foreground {
                    return ToolExecutionUpdate::Ignored;
                }
                execution.phase = ToolExecutionPhase::Background;
                let request = execution.request.clone();
                let task_id = task.task.task_id.clone();
                ToolExecutionUpdate::TaskCreated {
                    result: task_created_result(&request, &task_id),
                    event: ToolEvent::TaskCreated { request, task_id, status_message: task.task.status_message },
                }
            }
            ToolCallEvent::TaskStatus(task) => {
                let Some(execution) = self.executions.get(tool_id) else {
                    return ToolExecutionUpdate::Ignored;
                };
                if execution.phase != ToolExecutionPhase::Background {
                    return ToolExecutionUpdate::Ignored;
                }
                ToolExecutionUpdate::Event(ToolEvent::TaskStatus {
                    request: execution.request.clone(),
                    task_id: task.task_id,
                    status: task_status_name(task.status),
                    status_message: task.status_message,
                })
            }
            ToolCallEvent::TaskComplete { task, result } => {
                if self.take_retiring(tool_id).is_some() {
                    return ToolExecutionUpdate::Retired;
                }
                let Some(execution) = self.take_background(tool_id) else {
                    return ToolExecutionUpdate::Ignored;
                };
                ToolExecutionUpdate::TaskCompleted(map_task_result_to_outcome(execution.request, task, result))
            }
            ToolCallEvent::Cancelled { task_id } => {
                if self.take_retiring(tool_id).is_some() {
                    return ToolExecutionUpdate::Retired;
                }
                let Some(execution) = self.take_background(tool_id) else {
                    return ToolExecutionUpdate::Ignored;
                };
                ToolExecutionUpdate::TaskCancelled(TaskOutcome {
                    request: execution.request,
                    task_id: task_id.unwrap_or_else(|| UNASSIGNED_TASK_ID.to_string()),
                    state: TaskOutcomeState::Cancelled,
                })
            }
            ToolCallEvent::Complete(outcome) => {
                if self.take_retiring(tool_id).is_some() {
                    return ToolExecutionUpdate::Retired;
                }
                let Some(execution) = self.take_foreground(tool_id) else {
                    return ToolExecutionUpdate::Ignored;
                };
                match convert_tool_result(&execution.request, outcome) {
                    Ok((result, result_meta)) => ToolExecutionUpdate::Completed {
                        result: Ok(result.clone()),
                        event: ToolEvent::Result { result, result_meta },
                    },
                    Err(error) => {
                        ToolExecutionUpdate::Completed { result: Err(error.clone()), event: ToolEvent::Error { error } }
                    }
                }
            }
        }
    }

    pub(super) fn retire_foreground(&mut self) {
        for execution in self.executions.values_mut() {
            if execution.phase == ToolExecutionPhase::Foreground {
                execution.phase = ToolExecutionPhase::Retiring;
            }
        }
    }

    pub(super) fn abort(&mut self, policy: &ToolAbortPolicy) -> Vec<String> {
        let mut removed = Vec::new();
        self.executions.retain(|tool_id, execution| match execution.phase {
            ToolExecutionPhase::Background | ToolExecutionPhase::Cancelling => {
                execution.cancellation_token.cancel();
                execution.phase = match policy {
                    ToolAbortPolicy::PreserveBackgroundAcknowledgements => ToolExecutionPhase::Cancelling,
                    ToolAbortPolicy::CancelAll => ToolExecutionPhase::Retiring,
                };
                true
            }
            ToolExecutionPhase::Retiring if matches!(policy, ToolAbortPolicy::CancelAll) => true,
            ToolExecutionPhase::Foreground | ToolExecutionPhase::Retiring => {
                execution.cancellation_token.cancel();
                removed.push(tool_id.clone());
                false
            }
        });
        removed
    }

    fn take_retiring(&mut self, tool_id: &str) -> Option<ToolExecution> {
        if self.executions.get(tool_id)?.phase != ToolExecutionPhase::Retiring {
            return None;
        }
        self.executions.remove(tool_id)
    }

    fn take_foreground(&mut self, tool_id: &str) -> Option<ToolExecution> {
        if self.executions.get(tool_id)?.phase != ToolExecutionPhase::Foreground {
            return None;
        }
        self.executions.remove(tool_id)
    }

    fn take_background(&mut self, tool_id: &str) -> Option<ToolExecution> {
        let phase = self.executions.get(tool_id)?.phase;
        if !matches!(phase, ToolExecutionPhase::Background | ToolExecutionPhase::Cancelling) {
            return None;
        }
        self.executions.remove(tool_id)
    }
}

const UNASSIGNED_TASK_ID: &str = "pending";

/// Tools smuggle structured updates through the progress message string. Decode
/// them once here so every downstream consumer sees typed events.
fn progress_event(request: ToolCallRequest, progress: ProgressNotificationParam) -> ToolEvent {
    let ProgressNotificationParam { progress, total, message, .. } = progress;
    let Some(message) = message else {
        return ToolEvent::Progress { request, progress, total, message: None };
    };
    if let Ok(payload) = serde_json::from_str::<SubAgentProgressPayload>(&message) {
        return ToolEvent::SubAgentProgress { request, payload: Box::new(payload) };
    }
    if let Ok(meta) = serde_json::from_str::<ToolResultMeta>(&message) {
        return ToolEvent::DisplayUpdate { request, meta };
    }
    ToolEvent::Progress { request, progress, total, message: Some(message) }
}

fn task_status_name(status: rmcp::model::TaskStatus) -> String {
    use rmcp::model::TaskStatus;
    match status {
        TaskStatus::Working => "working",
        TaskStatus::InputRequired => "input_required",
        TaskStatus::Completed => "completed",
        TaskStatus::Failed => "failed",
        TaskStatus::Cancelled => "cancelled",
        _ => "unknown",
    }
    .to_string()
}
