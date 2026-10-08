use super::tool_call::{ToolCallError, ToolCallEvent};
use crate::client::McpClient;
use crate::client::handler::UnsupportedInput;
use async_stream::stream;
use futures::{Stream, StreamExt};
use rmcp::model::{
    CancelTaskParams, CreateTaskResult, GetTaskParams, InputRequests, ProgressNotificationParam, Task, TaskPayload,
    TaskStatus, UpdateTaskParams,
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
    #[error("returned a task payload this client does not support (status {status:?})")]
    UnsupportedPayload { status: TaskStatus },
}

pub(super) fn stream_task_events<'a>(
    client: &'a McpClient,
    created: CreateTaskResult,
    progress: impl Stream<Item = ProgressNotificationParam> + 'a,
) -> impl Stream<Item = ToolCallEvent> + 'a {
    stream! {
        yield ToolCallEvent::TaskCreated(created.clone());
        let mut progress = pin!(progress);
        let mut polled = pin!(poll_task(client, created.task));

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

pub(super) async fn cancel_server_task(client: &McpClient, task_id: &str) {
    let server_name = client.name();
    match timeout(Duration::from_secs(1), client.peer().cancel_task(CancelTaskParams::new(task_id))).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            tracing::warn!(server = %server_name, %task_id, "Failed to cancel abandoned MCP task: {error}");
        }
        Err(_) => tracing::warn!(server = %server_name, %task_id, "Timed out cancelling abandoned MCP task"),
    }
}

fn poll_task<'a>(client: &'a McpClient, mut task: Task) -> impl Stream<Item = ToolCallEvent> + 'a {
    stream! {
        let mut answered = HashSet::new();

        loop {
            if is_task_expired(&task) {
                yield failed(task, TaskErrorReason::Expired);
                return;
            }

            let detailed_task = match client.peer().get_task(GetTaskParams::new(task.task_id.clone())).await {
                Ok(result) => result.task,
                Err(error) => {
                    yield abandoned(client, task, TaskErrorReason::Get(error)).await;
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
                    if let Err(error) = answer_inputs(client, &task.task_id, input_requests, &mut answered).await {
                        cancel_server_task(client, &task.task_id).await;
                        yield ToolCallEvent::Done { task: Some(task), result: Err(error) };
                        return;
                    }
                }
                TaskPayload::Completed { result } => {
                    let result = serde_json::from_value(serde_json::Value::Object(result))
                        .map_err(|source| task_error(&task.task_id, TaskErrorReason::MalformedResult(source)));
                    yield ToolCallEvent::Done { task: Some(task), result };
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

            sleep(task.poll_interval_ms.map_or(DEFAULT_POLL_INTERVAL, Duration::from_millis)).await;
        }
    }
}

const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(1);

async fn answer_inputs(
    client: &McpClient,
    task_id: &str,
    input_requests: InputRequests,
    answered: &mut HashSet<String>,
) -> Result<(), ToolCallError> {
    if input_requests.keys().any(|key| answered.contains(key)) {
        return Err(task_error(task_id, TaskErrorReason::RepeatedInput));
    }
    let (responses, _) = client
        .handler()
        .elicit_inputs(input_requests)
        .await
        .map_err(|UnsupportedInput| ToolCallError::UnsupportedInput)?;
    answered.extend(responses.keys().cloned());
    client
        .peer()
        .update_task(UpdateTaskParams::new(task_id, responses))
        .await
        .map_err(|error| task_error(task_id, TaskErrorReason::Update(error)))
}

async fn abandoned(client: &McpClient, task: Task, reason: TaskErrorReason) -> ToolCallEvent {
    cancel_server_task(client, &task.task_id).await;
    failed(task, reason)
}

fn failed(task: Task, reason: TaskErrorReason) -> ToolCallEvent {
    let error = task_error(&task.task_id, reason);
    ToolCallEvent::Done { task: Some(task), result: Err(error) }
}

fn task_error(task_id: &str, reason: TaskErrorReason) -> ToolCallError {
    ToolCallError::Task { task_id: task_id.to_string(), reason: Box::new(reason) }
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
