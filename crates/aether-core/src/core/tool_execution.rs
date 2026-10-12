use crate::events::{SubAgentProgressPayload, TaskOutcome, ToolEvent, task_created_result};
use crate::mcp::tool_bridge::{ToolRejection, convert_tool_result, map_task_result_to_outcome};
use llm::{ToolCallError, ToolCallRequest, ToolCallResult};
use mcp_utils::client::{CancellationToken, ToolCallEvent};
use rmcp::model::ProgressNotificationParam;
use std::collections::HashMap;
use utils::display_meta::ToolResultMeta;
use utils::temp_dir::TempDir;

#[derive(Default)]
pub(super) struct ToolExecutions {
    executions: HashMap<String, ToolExecution>,
    spill_dir: TempDir,
}

pub(super) enum ToolExecutionUpdate {
    Event(ToolEvent),
    TaskCreated { result: ToolCallResult, event: ToolEvent },
    Completed { result: Result<ToolCallResult, ToolCallError>, event: ToolEvent },
    TaskFinished(TaskOutcome),
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

#[derive(PartialEq, Eq)]
enum ToolExecutionPhase {
    Foreground,
    Background { task_id: String, cancelling: bool },
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
                    matches!(
                        execution.phase,
                        ToolExecutionPhase::Foreground | ToolExecutionPhase::Background { cancelling: false, .. }
                    )
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
                let task_id = task.task.task_id.clone();
                execution.phase = ToolExecutionPhase::Background { task_id: task_id.clone(), cancelling: false };
                let request = execution.request.clone();
                ToolExecutionUpdate::TaskCreated {
                    result: task_created_result(&request, &task_id),
                    event: ToolEvent::TaskCreated { request, task_id, status_message: task.task.status_message },
                }
            }
            ToolCallEvent::TaskStatus(task) => {
                let Some(execution) = self.executions.get(tool_id) else {
                    return ToolExecutionUpdate::Ignored;
                };
                if !matches!(execution.phase, ToolExecutionPhase::Background { cancelling: false, .. }) {
                    return ToolExecutionUpdate::Ignored;
                }
                ToolExecutionUpdate::Event(ToolEvent::TaskStatus {
                    request: execution.request.clone(),
                    task_id: task.task_id,
                    status: task_status_name(task.status),
                    status_message: task.status_message,
                })
            }
            ToolCallEvent::Done(result) => {
                let Some(ToolExecution { request, phase, .. }) = self.executions.remove(tool_id) else {
                    return ToolExecutionUpdate::Ignored;
                };
                match phase {
                    ToolExecutionPhase::Retiring => ToolExecutionUpdate::Retired,
                    ToolExecutionPhase::Foreground => match convert_tool_result(&request, result, &self.spill_dir) {
                        Ok((result, result_meta)) => ToolExecutionUpdate::Completed {
                            result: Ok(result.clone()),
                            event: ToolEvent::Result { result, result_meta },
                        },
                        Err(error) => ToolExecutionUpdate::Completed {
                            result: Err(error.clone()),
                            event: ToolEvent::Error { error },
                        },
                    },
                    ToolExecutionPhase::Background { task_id, .. } => {
                        let outcome = map_task_result_to_outcome(request, task_id, result, &self.spill_dir);
                        ToolExecutionUpdate::TaskFinished(outcome)
                    }
                }
            }
        }
    }

    /// Fails a tool call that never reached a server.
    pub(super) fn reject(&mut self, tool_id: &str, rejection: &ToolRejection) -> ToolExecutionUpdate {
        let Some(execution) = self.executions.remove(tool_id) else {
            return ToolExecutionUpdate::Ignored;
        };
        let error = ToolCallError::from_request(&execution.request, rejection.to_string());
        ToolExecutionUpdate::Completed { result: Err(error.clone()), event: ToolEvent::Error { error } }
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
        self.executions.retain(|tool_id, execution| match &mut execution.phase {
            ToolExecutionPhase::Background { cancelling, .. } => {
                execution.cancellation_token.cancel();
                match policy {
                    ToolAbortPolicy::PreserveBackgroundAcknowledgements => *cancelling = true,
                    ToolAbortPolicy::CancelAll => execution.phase = ToolExecutionPhase::Retiring,
                }
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
}

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
