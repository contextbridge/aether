use super::bounds::{Bounds, Interrupted, Stop};
use super::mrtr::{AbortReason, MrtrAction, MrtrState};
use super::task::{TaskErrorReason, task_events};
use crate::McpError;
use crate::client::McpClient;
use crate::client::handler::UnsupportedInput;
use async_stream::stream;
use futures::{Stream, StreamExt};
use rmcp::RoleClient;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ClientRequest, CreateTaskResult, InputRequiredResult,
    ProgressNotificationParam, Request, RequestMetaObject, ServerResult, Task,
};
use rmcp::service::{PeerRequestOptions, RequestHandle, ServiceError};
use std::pin::{Pin, pin};
use std::task::{Context, Poll};
use std::time::Duration;
use thiserror::Error;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

pub struct ToolCall {
    events: Pin<Box<dyn Stream<Item = ToolCallEvent> + Send>>,
}

#[derive(Debug)]
pub enum ToolCallEvent {
    Progress(ProgressNotificationParam),
    TaskCreated(CreateTaskResult),
    TaskStatus(Task),
    TaskComplete { task: Task, result: Result<CallToolResult, ToolCallError> },
    Complete(Result<CallToolResult, ToolCallError>),
    Cancelled { task_id: Option<String> },
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
    #[error("Server {0}")]
    Aborted(#[source] AbortReason),
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
    pub(crate) fn new(client: McpClient, mut params: CallToolRequestParams, options: ToolCallOptions) -> Self {
        let events = stream! {
            let bounds = Bounds::new(options.timeout, options.cancel.clone());
            let mut mrtr_state = MrtrState::new();

            loop {
                let request = ClientRequest::CallToolRequest(Request::new(params.clone()));
                let handle = match bounds.try_run(client.peer().send_cancellable_request(request, peer_request_options(&options))).await {
                    Ok(handle) => handle,
                    Err(stop) => {
                        yield stop.map_failure(ToolCallError::Send).into();
                        return;
                    }
                };

                let mut progress = client.handler().progress.subscribe(handle.progress_token.clone()).await;
                let mut pending = pin!(bounds.run(await_tool_response(handle)));
                let bounded = loop {
                    tokio::select! {
                        biased;
                        progress_event = progress.next() => match progress_event {
                            Some(progress_event) => yield ToolCallEvent::Progress(progress_event),
                            None => break pending.await,
                        },
                        bounded = pending.as_mut() => break bounded,
                    }
                };

                let response = match bounded {
                    Ok(response) => response,
                    Err(interrupted) => {
                        yield interrupted.into();
                        return;
                    }
                };

                match response {
                    Ok(CallToolResponse::Complete(result)) => {
                        yield ToolCallEvent::Complete(Ok(result));
                        return;
                    }
                    Ok(CallToolResponse::InputRequired(input_required)) => match mrtr_state.tick(input_required) {
                        MrtrAction::Poll { backoff, request_state } => {
                            if let Err(interrupted) = bounds.run(sleep(backoff)).await {
                                yield interrupted.into();
                                return;
                            }
                            params.input_responses = None;
                            params.request_state = Some(request_state);
                        }
                        MrtrAction::Elicit { input_requests, request_state } => {
                            match bounds.try_run(client.handler().elicit_inputs(input_requests)).await {
                                Ok((responses, cancelled)) => {
                                    mrtr_state.record_cancelled(cancelled);
                                    params.input_responses = Some(responses);
                                    params.request_state = request_state;
                                }
                                Err(stop) => {
                                    yield stop.map_failure(|UnsupportedInput| ToolCallError::UnsupportedInput).into();
                                    return;
                                }
                            }
                        }
                        MrtrAction::Abort(reason) => {
                            yield ToolCallError::Aborted(reason).into();
                            return;
                        }
                    },
                    Ok(CallToolResponse::Task(created)) => {
                        let mut events = pin!(task_events(&client, &bounds, created, progress));
                        while let Some(event) = events.next().await {
                            yield event;
                        }
                        return;
                    }
                    Ok(_) => {
                        yield ToolCallError::UnsupportedResponse.into();
                        return;
                    }
                    Err(e) => {
                        yield ToolCallError::Call(e).into();
                        return;
                    }
                }
            }
        };

        Self { events: Box::pin(events) }
    }

    pub fn failed(error: ToolCallError) -> Self {
        Self { events: Box::pin(futures::stream::once(std::future::ready(ToolCallEvent::from(error)))) }
    }

    pub async fn result(mut self) -> Result<CallToolResult, ToolCallError> {
        while let Some(event) = self.next().await {
            match event {
                ToolCallEvent::Complete(result) | ToolCallEvent::TaskComplete { result, .. } => return result,
                ToolCallEvent::Cancelled { .. } => return Err(ToolCallError::Cancelled),
                ToolCallEvent::Progress(_) | ToolCallEvent::TaskCreated(_) | ToolCallEvent::TaskStatus(_) => {}
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

impl From<Interrupted> for ToolCallEvent {
    fn from(interrupted: Interrupted) -> Self {
        match interrupted {
            Interrupted::TimedOut(timeout) => ToolCallError::TimedOut(timeout).into(),
            Interrupted::Cancelled => Self::Cancelled { task_id: None },
        }
    }
}

impl From<Stop<ToolCallError>> for ToolCallEvent {
    fn from(stop: Stop<ToolCallError>) -> Self {
        match stop {
            Stop::Interrupted(interrupted) => interrupted.into(),
            Stop::Failed(error) => error.into(),
        }
    }
}

impl From<ToolCallError> for ToolCallEvent {
    fn from(error: ToolCallError) -> Self {
        Self::Complete(Err(error))
    }
}

fn peer_request_options(options: &ToolCallOptions) -> PeerRequestOptions {
    options
        .meta
        .clone()
        .map_or_else(PeerRequestOptions::no_options, |meta| PeerRequestOptions::no_options().with_meta(meta))
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
