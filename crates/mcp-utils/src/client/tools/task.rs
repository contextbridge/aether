use super::bounds::{Bounds, Interrupted, Stop};
use super::tool_call::{ToolCallError, ToolCallEvent};
use crate::client::McpClient;
use crate::client::handler::UnsupportedInput;
use async_stream::stream;
use futures::{Stream, StreamExt};
use rmcp::model::{
    CancelTaskParams, CreateTaskResult, GetTaskParams, InputRequests, InputResponses, ProgressNotificationParam, Task,
    TaskPayload, TaskStatus, UpdateTaskParams,
};
use rmcp::service::ServiceError;
use std::collections::HashSet;
use std::pin::pin;
use std::time::Duration;
use thiserror::Error;
use tokio::time::{sleep, timeout};

#[derive(Debug, Error)]
pub enum TaskErrorReason {
    #[error("failed to get task: {0}")]
    Get(#[source] ServiceError),
    #[error("failed to update task: {0}")]
    Update(#[source] ServiceError),
    #[error("expired before completion")]
    Expired,
    #[error("repeated input requests that were already answered")]
    RepeatedInput,
    #[error("failed: {error}")]
    Failed { error: serde_json::Value },
    #[error("was cancelled")]
    Cancelled,
    #[error("returned a malformed result: {0}")]
    MalformedResult(#[source] serde_json::Error),
    #[error("requested an input kind this client does not support (sampling or roots)")]
    UnsupportedInput,
    #[error("returned a task payload this client does not support (status {status:?})")]
    UnsupportedPayload { status: TaskStatus },
}

pub(super) fn task_events<'a>(
    client: &'a McpClient,
    bounds: &'a Bounds,
    created: CreateTaskResult,
    progress: impl Stream<Item = ProgressNotificationParam> + 'a,
) -> impl Stream<Item = ToolCallEvent> + 'a {
    stream! {
        yield ToolCallEvent::TaskCreated(created.clone());
        let mut progress = pin!(progress);
        let mut polled = pin!(poll_task(client, bounds, created.task));

        loop {
            tokio::select! {
                biased;
                progress_event = progress.next() => match progress_event {
                    Some(progress_event) => yield ToolCallEvent::Progress(progress_event),
                    None => {
                        while let Some(event) = polled.next().await {
                            yield event;
                        }
                        return;
                    }
                },
                event = polled.next() => match event {
                    Some(event) => yield event,
                    None => return,
                },
            }
        }
    }
}

fn poll_task<'a>(client: &'a McpClient, bounds: &'a Bounds, mut task: Task) -> impl Stream<Item = ToolCallEvent> + 'a {
    stream! {
        let mut input_state = TaskInputState::default();

        loop {
            if is_task_expired(&task) {
                yield failed(task, TaskErrorReason::Expired);
                return;
            }

            let params = GetTaskParams::new(task.task_id.clone());
            let detailed_task = match bounds.try_run(client.peer().get_task(params)).await {
                Ok(result) => result.task,
                Err(stop) => {
                    yield stopped(client, task, stop.map_failure(TaskErrorReason::Get)).await;
                    return;
                }
            };

            task = detailed_task.task;
            if !task.status.is_terminal() {
                yield ToolCallEvent::TaskStatus(task.clone());
            }

            match detailed_task.payload {
                TaskPayload::Working => {}
                TaskPayload::InputRequired { input_requests } => {
                    let answered = answer_inputs(client, &task.task_id, input_requests, &mut input_state);
                    if let Err(stop) = bounds.try_run(answered).await {
                        yield stopped(client, task, stop).await;
                        return;
                    }
                }
                TaskPayload::Completed { result } => {
                    let result = serde_json::from_value(serde_json::Value::Object(result))
                        .map_err(|source| task_error(&task.task_id, TaskErrorReason::MalformedResult(source)));
                    yield ToolCallEvent::TaskComplete { task, result };
                    return;
                }
                TaskPayload::Failed { error } => {
                    yield failed(task, TaskErrorReason::Failed { error: serde_json::Value::Object(error) });
                    return;
                }
                TaskPayload::Cancelled => {
                    yield failed(task, TaskErrorReason::Cancelled);
                    return;
                }
                _ => {
                    let status = task.status;
                    yield abandoned(client, task, TaskErrorReason::UnsupportedPayload { status }).await;
                    return;
                }
            }

            let poll_interval = task.poll_interval_ms.map_or(DEFAULT_POLL_INTERVAL, Duration::from_millis);
            if let Err(interrupted) = bounds.run(sleep(poll_interval)).await {
                yield stopped(client, task, Stop::Interrupted(interrupted)).await;
                return;
            }
        }
    }
}

const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Default)]
struct TaskInputState {
    answered_keys: HashSet<String>,
}

impl TaskInputState {
    fn check(&self, input_requests: &InputRequests) -> Result<(), TaskErrorReason> {
        if input_requests.keys().any(|key| self.answered_keys.contains(key)) {
            return Err(TaskErrorReason::RepeatedInput);
        }
        Ok(())
    }

    fn record_answered(&mut self, responses: &InputResponses) {
        self.answered_keys.extend(responses.keys().cloned());
    }
}

async fn answer_inputs(
    client: &McpClient,
    task_id: &str,
    input_requests: InputRequests,
    input_state: &mut TaskInputState,
) -> Result<(), TaskErrorReason> {
    input_state.check(&input_requests)?;
    let (responses, _) = client
        .handler()
        .elicit_inputs(input_requests)
        .await
        .map_err(|UnsupportedInput| TaskErrorReason::UnsupportedInput)?;
    input_state.record_answered(&responses);
    client.peer().update_task(UpdateTaskParams::new(task_id, responses)).await.map_err(TaskErrorReason::Update)
}

async fn stopped(client: &McpClient, task: Task, stop: Stop<TaskErrorReason>) -> ToolCallEvent {
    match stop {
        Stop::Interrupted(Interrupted::TimedOut(timeout)) => {
            cancel_server_task(client, &task.task_id).await;
            ToolCallEvent::TaskComplete { task, result: Err(ToolCallError::TimedOut(timeout)) }
        }
        Stop::Interrupted(Interrupted::Cancelled) => {
            cancel_server_task(client, &task.task_id).await;
            ToolCallEvent::Cancelled { task_id: Some(task.task_id) }
        }
        Stop::Failed(reason) => abandoned(client, task, reason).await,
    }
}

async fn abandoned(client: &McpClient, task: Task, reason: TaskErrorReason) -> ToolCallEvent {
    cancel_server_task(client, &task.task_id).await;
    failed(task, reason)
}

fn failed(task: Task, reason: TaskErrorReason) -> ToolCallEvent {
    let error = task_error(&task.task_id, reason);
    ToolCallEvent::TaskComplete { task, result: Err(error) }
}

fn task_error(task_id: &str, reason: TaskErrorReason) -> ToolCallError {
    ToolCallError::Task { task_id: task_id.to_string(), reason: Box::new(reason) }
}

async fn cancel_server_task(client: &McpClient, task_id: &str) {
    let server_name = client.name();
    match timeout(Duration::from_secs(1), client.peer().cancel_task(CancelTaskParams::new(task_id))).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            tracing::warn!(server = %server_name, %task_id, "Failed to cancel abandoned MCP task: {error}");
        }
        Err(_) => tracing::warn!(server = %server_name, %task_id, "Timed out cancelling abandoned MCP task"),
    }
}

fn is_task_expired(task: &Task) -> bool {
    if task.status.is_terminal() {
        return false;
    }
    let Some(ttl_ms) = task.ttl_ms else {
        return false;
    };
    let Ok(created_at) = chrono::DateTime::parse_from_rfc3339(&task.created_at) else {
        tracing::warn!(task_id = %task.task_id, created_at = %task.created_at, "Ignoring malformed MCP task creation timestamp");
        return false;
    };
    let Ok(ttl_ms) = i64::try_from(ttl_ms) else {
        return false;
    };
    created_at
        .with_timezone(&chrono::Utc)
        .checked_add_signed(chrono::Duration::milliseconds(ttl_ms))
        .is_some_and(|expires_at| chrono::Utc::now() > expires_at)
}
