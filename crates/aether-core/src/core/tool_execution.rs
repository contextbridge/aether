use crate::events::{TaskOutcome, TaskOutcomeState, ToolEvent, task_created_result};
use crate::mcp::tool_bridge::{convert_tool_result, map_task_result_to_outcome};
use llm::{ToolCallError, ToolCallRequest, ToolCallResult};
use mcp_utils::client::{CancellationToken, ToolCallEvent};
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

    pub(super) fn on_event(&mut self, tool_id: &str, event: ToolCallEvent) -> ToolExecutionUpdate {
        match event {
            ToolCallEvent::Progress(progress) => {
                let Some(execution) = self.foreground(tool_id) else {
                    return ToolExecutionUpdate::Ignored;
                };
                ToolExecutionUpdate::Event(ToolEvent::Progress {
                    request: execution.request.clone(),
                    progress: progress.progress,
                    total: progress.total,
                    message: progress.message,
                })
            }
            ToolCallEvent::TaskCreated(task) => {
                let Some(execution) = self.executions.get_mut(tool_id) else {
                    return ToolExecutionUpdate::Ignored;
                };
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
                let Some(execution) = self.take_background(tool_id) else {
                    return ToolExecutionUpdate::Ignored;
                };
                ToolExecutionUpdate::TaskCompleted(map_task_result_to_outcome(execution.request, task, result))
            }
            ToolCallEvent::Cancelled { task_id } => {
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

    pub(super) fn cancel_foreground(&mut self) -> Vec<String> {
        let mut removed = Vec::new();
        self.executions.retain(|tool_id, execution| {
            if execution.phase != ToolExecutionPhase::Foreground {
                return true;
            }
            execution.cancellation_token.cancel();
            removed.push(tool_id.clone());
            false
        });
        removed
    }

    pub(super) fn abort(&mut self, policy: ToolAbortPolicy) -> Vec<String> {
        let mut removed = Vec::new();
        self.executions.retain(|tool_id, execution| {
            execution.cancellation_token.cancel();
            let preserve = matches!(policy, ToolAbortPolicy::PreserveBackgroundAcknowledgements)
                && matches!(execution.phase, ToolExecutionPhase::Background | ToolExecutionPhase::Cancelling);
            if preserve {
                execution.phase = ToolExecutionPhase::Cancelling;
            } else {
                removed.push(tool_id.clone());
            }
            preserve
        });
        removed
    }

    fn foreground(&self, tool_id: &str) -> Option<&ToolExecution> {
        self.executions.get(tool_id).filter(|execution| execution.phase == ToolExecutionPhase::Foreground)
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
