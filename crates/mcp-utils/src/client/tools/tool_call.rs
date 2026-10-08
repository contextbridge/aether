use super::mrtr::{MrtrAction, MrtrState};
use super::task::{TaskErrorReason, cancel_server_task, stream_task_events};
use crate::McpError;
use crate::client::McpClient;
use async_stream::stream;
use futures::{Stream, StreamExt};
use rmcp::RoleClient;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ClientRequest, CreateTaskResult, DEFAULT_MRTR_MAX_ROUNDS,
    InputRequiredResult, ProgressNotificationParam, Request, RequestMetaObject, ServerResult, Task,
};
use rmcp::service::{PeerRequestOptions, RequestHandle, ServiceError};
use std::future::pending;
use std::pin::{Pin, pin};
use std::task::{Context, Poll};
use std::time::Duration;
use thiserror::Error;
use tokio::time::{Instant, sleep, sleep_until};
use tokio_util::sync::CancellationToken;

pub struct ToolCall {
    events: Pin<Box<dyn Stream<Item = ToolCallEvent> + Send>>,
}

#[derive(Debug)]
pub enum ToolCallEvent {
    Progress(ProgressNotificationParam),
    TaskCreated(CreateTaskResult),
    TaskStatus(Task),
    Done { task: Option<Task>, result: Result<CallToolResult, ToolCallError> },
}

#[derive(Debug, Clone, Default)]
pub struct ToolCallOptions {
    pub timeout: Option<Duration>,
    pub meta: Option<RequestMetaObject>,
    pub cancel: CancellationToken,
}

#[derive(Debug, Error)]
pub enum ToolCallError {
    #[error("Failed to send tool request: {0}")]
    Send(#[source] ServiceError),
    #[error("Tool execution failed: {0}")]
    Call(#[source] ServiceError),
    #[error("Tool call exceeded its {0:?} timeout")]
    TimedOut(Duration),
    #[error("Server requested an input kind this client does not support (sampling or roots)")]
    UnsupportedInput,
    #[error("Server requested input without any input requests or request state")]
    EmptyInputRequired,
    #[error("Server requested input again after the user cancelled")]
    RePromptAfterCancel,
    #[error("Server did not complete within {DEFAULT_MRTR_MAX_ROUNDS} MRTR input rounds")]
    InputRoundsExceeded,
    #[error("Task '{task_id}' {reason}")]
    Task {
        task_id: String,
        #[source]
        reason: Box<TaskErrorReason>,
    },
    #[error("Server returned a tool call response kind this client does not support")]
    UnsupportedResponse,
    #[error("Tool call was cancelled")]
    Cancelled,
    #[error("Invalid tool arguments: {0}")]
    InvalidArguments(#[source] serde_json::Error),
    #[error("Failed to resolve tool: {0}")]
    Unresolved(#[source] McpError),
}

impl ToolCallOptions {
    pub fn with_timeout(timeout: Duration) -> Self {
        Self { timeout: Some(timeout), ..Self::default() }
    }
}

impl ToolCall {
    pub(crate) fn new(client: McpClient, params: CallToolRequestParams, options: ToolCallOptions) -> Self {
        let ToolCallOptions { timeout, meta, cancel } = options;
        let events = stream_events(client.clone(), params, meta);
        Self { events: Box::pin(bounded(client, events, timeout, cancel)) }
    }

    pub fn failed(error: ToolCallError) -> Self {
        Self { events: Box::pin(futures::stream::once(std::future::ready(ToolCallEvent::from(error)))) }
    }

    pub async fn result(mut self) -> Result<CallToolResult, ToolCallError> {
        while let Some(event) = self.next().await {
            if let ToolCallEvent::Done { result, .. } = event {
                return result;
            }
        }
        Err(ToolCallError::Cancelled)
    }
}

impl Stream for ToolCall {
    type Item = ToolCallEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.events.as_mut().poll_next(cx)
    }
}

impl From<ToolCallError> for ToolCallEvent {
    fn from(error: ToolCallError) -> Self {
        Self::Done { task: None, result: Err(error) }
    }
}

fn bounded(
    client: McpClient,
    events: impl Stream<Item = ToolCallEvent> + Send + 'static,
    timeout: Option<Duration>,
    cancel: CancellationToken,
) -> impl Stream<Item = ToolCallEvent> + Send + 'static {
    stream! {
        let mut events = Box::pin(events);
        let mut expired = pin!(expiry(timeout));
        let mut cancelled = pin!(cancel.cancelled());
        let mut task = None;

        let interrupted = loop {
            tokio::select! {
                biased;
                event = events.next() => {
                    let Some(event) = event else { return };
                    match &event {
                        ToolCallEvent::TaskCreated(created) => task = Some(created.task.clone()),
                        ToolCallEvent::TaskStatus(status) => task = Some(status.clone()),
                        ToolCallEvent::Progress(_) | ToolCallEvent::Done { .. } => {}
                    }
                    let done = matches!(event, ToolCallEvent::Done { .. });
                    yield event;
                    if done {
                        return;
                    }
                }
                timeout = &mut expired => break ToolCallError::TimedOut(timeout),
                () = &mut cancelled => break ToolCallError::Cancelled,
            }
        };

        drop(events);
        if let Some(task) = &task {
            cancel_server_task(&client, &task.task_id).await;
        }
        yield ToolCallEvent::Done { task, result: Err(interrupted) };
    }
}

fn stream_events(
    client: McpClient,
    mut params: CallToolRequestParams,
    meta: Option<RequestMetaObject>,
) -> impl Stream<Item = ToolCallEvent> + Send + 'static {
    stream! {
        let mut mrtr_state = MrtrState::new();

        loop {
            let request = ClientRequest::CallToolRequest(Request::new(params.clone()));
            let handle = match client.peer().send_cancellable_request(request, peer_request_options(meta.clone())).await {
                Ok(handle) => handle,
                Err(error) => {
                    yield ToolCallError::Send(error).into();
                    return;
                }
            };

            let mut progress = client.handler().progress.subscribe(handle.progress_token.clone()).await;
            let mut pending = pin!(await_tool_response(handle));
            let response = loop {
                tokio::select! {
                    biased;
                    progress_event = progress.next() => match progress_event {
                        Some(progress_event) => yield ToolCallEvent::Progress(progress_event),
                        None => break pending.await,
                    },
                    response = pending.as_mut() => break response,
                }
            };

            match response {
                Ok(CallToolResponse::Complete(result)) => {
                    yield ToolCallEvent::Done { task: None, result: Ok(result) };
                    return;
                }
                Ok(CallToolResponse::InputRequired(input_required)) => match mrtr_state.tick(input_required) {
                    MrtrAction::Poll { backoff, request_state } => {
                        sleep(backoff).await;
                        params.input_responses = None;
                        params.request_state = Some(request_state);
                    }
                    MrtrAction::Elicit { input_requests, request_state } => {
                        let Ok((responses, cancelled)) = client.handler().elicit_inputs(input_requests).await else {
                            yield ToolCallError::UnsupportedInput.into();
                            return;
                        };
                        mrtr_state.record_cancelled(cancelled);
                        params.input_responses = Some(responses);
                        params.request_state = request_state;
                    }
                    MrtrAction::Abort(error) => {
                        yield error.into();
                        return;
                    }
                },
                Ok(CallToolResponse::Task(created)) => {
                    let mut events = pin!(stream_task_events(&client, created, progress));
                    while let Some(event) = events.next().await {
                        yield event;
                    }
                    return;
                }
                Ok(_) => {
                    yield ToolCallError::UnsupportedResponse.into();
                    return;
                }
                Err(error) => {
                    yield ToolCallError::Call(error).into();
                    return;
                }
            }
        }
    }
}

async fn expiry(timeout: Option<Duration>) -> Duration {
    match timeout.and_then(|timeout| Some((Instant::now().checked_add(timeout)?, timeout))) {
        Some((deadline, timeout)) => {
            sleep_until(deadline).await;
            timeout
        }
        None => pending().await,
    }
}

fn peer_request_options(meta: Option<RequestMetaObject>) -> PeerRequestOptions {
    meta.map_or_else(PeerRequestOptions::no_options, |meta| PeerRequestOptions::no_options().with_meta(meta))
}

async fn await_tool_response(handle: RequestHandle<RoleClient>) -> Result<CallToolResponse, ServiceError> {
    match handle.await_response().await? {
        ServerResult::CallToolResult(result) => Ok(CallToolResponse::Complete(result)),
        ServerResult::InputRequiredResult(result) => Ok(CallToolResponse::InputRequired(result)),
        ServerResult::CreateTaskResult(result) => Ok(CallToolResponse::Task(result)),
        ServerResult::CustomResult(result)
            if result.0.get("resultType").and_then(serde_json::Value::as_str) == Some("input_required")
                && result.0.get("inputRequests").is_none()
                && result.0.get("requestState").is_none() =>
        {
            Ok(CallToolResponse::InputRequired(InputRequiredResult::new(None, None)))
        }
        _ => Err(ServiceError::UnexpectedResponse),
    }
}
