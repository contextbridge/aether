use crate::client::McpClient;
use crate::client::elicitation::{ElicitInputsError, elicit_inputs};
use crate::client::mrtr::{AbortReason, MrtrAction, MrtrState};
use crate::client::task::{TaskDriver, TaskErrorReason};
use async_stream::stream;
use futures::Stream;
use futures::StreamExt;
use futures::future::{Either, select};
use rmcp::RoleClient;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ClientRequest, CreateTaskResult, InputRequests,
    InputResponses, ProgressNotificationParam, Request, RequestMetaObject, ServerResult, Task,
};
use rmcp::service::{PeerRequestOptions, RequestHandle, RunningService, ServiceError};
use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Default)]
pub struct CallToolOptions {
    pub timeout: Duration,
    pub meta: Option<RequestMetaObject>,
    pub cancel: CancellationToken,
}

#[derive(Debug)]
pub enum ToolCallEvent {
    Progress(ProgressNotificationParam),
    TaskCreated(CreateTaskResult),
    TaskStatus(Task),
    Complete(Result<CallToolResult, CallToolError>),
    TaskComplete { task: Task, result: Result<CallToolResult, CallToolError> },
    Cancelled { task_id: Option<String> },
}

#[derive(Debug, Error)]
pub enum CallToolError {
    #[error("Failed to send tool request: {0}")]
    Send(#[source] ServiceError),
    #[error("Tool execution failed: {0}")]
    Call(#[source] ServiceError),
    #[error("Server '{server}' requested an input kind this client does not support (sampling or roots)")]
    UnsupportedInput { server: String },
    #[error("Server '{server}' failed to serialize an elicitation response: {source}")]
    SerializeResponse {
        server: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("{}", reason.message(server, *timeout))]
    Aborted { server: String, reason: AbortReason, timeout: Duration },
    #[error("Task '{task_id}' from server '{server}' {reason}")]
    Task {
        server: String,
        task_id: String,
        #[source]
        reason: Box<TaskErrorReason>,
    },
    #[error("Server '{server}' returned a tool call response kind this client does not support")]
    UnsupportedResponse { server: String },
    #[error("{message}")]
    Unavailable { message: String },
}

pub fn call_tool(
    client: Arc<RunningService<RoleClient, McpClient>>,
    mut params: CallToolRequestParams,
    options: CallToolOptions,
) -> impl Stream<Item = ToolCallEvent> {
    stream! {
        let server_name = client.service().server_name().to_string();
        let mut mrtr_state = MrtrState::new(options.timeout);

        loop {
            let request = ClientRequest::CallToolRequest(Request::new(params.clone()));
            let send = pin!(client.send_cancellable_request(request, peer_request_options(&options)));
            let handle = match select(send, pin!(options.cancel.cancelled())).await {
                Either::Left((Ok(handle), _)) => handle,
                Either::Left((Err(e), _)) => {
                    yield ToolCallEvent::Complete(Err(CallToolError::Send(e)));
                    return;
                }
                Either::Right(((), _)) => {
                    yield ToolCallEvent::Cancelled { task_id: None };
                    return;
                }
            };

            let mut progress = client.service().progress_dispatcher.subscribe(handle.progress_token.clone()).await;
            let mut response_or_cancel = pin!(await_response_or_cancel(handle, &options.cancel));
            let response = loop {
                match select(progress.next(), response_or_cancel.as_mut()).await {
                    Either::Left((Some(progress), _)) => yield ToolCallEvent::Progress(progress),
                    Either::Left((None, response_or_cancel)) => break response_or_cancel.await,
                    Either::Right((response, _)) => break response,
                }
            };
            let Some(response) = response else {
                yield ToolCallEvent::Cancelled { task_id: None };
                return;
            };

            match response {
                Ok(CallToolResponse::Complete(result)) => {
                    yield ToolCallEvent::Complete(Ok(result));
                    return;
                }
                Ok(CallToolResponse::InputRequired(input_required)) => {
                    match mrtr_state.tick(input_required) {
                        MrtrAction::Poll { backoff, request_state } => {
                            let backoff = pin!(sleep(backoff));
                            if let Either::Right(((), _)) = select(backoff, pin!(options.cancel.cancelled())).await {
                                yield ToolCallEvent::Cancelled { task_id: None };
                                return;
                            }
                            params.input_responses = None;
                            params.request_state = Some(request_state);
                        }
                        MrtrAction::Elicit { input_requests, request_state } => {
                            let elicit = pin!(elicit_input(client.service(), &mut mrtr_state, &server_name, input_requests));
                            match select(elicit, pin!(options.cancel.cancelled())).await {
                                Either::Left((Ok(responses), _)) => {
                                    params.input_responses = Some(responses);
                                    params.request_state = request_state;
                                }
                                Either::Left((Err(e), _)) => {
                                    yield ToolCallEvent::Complete(Err(e));
                                    return;
                                }
                                Either::Right(((), _)) => {
                                    yield ToolCallEvent::Cancelled { task_id: None };
                                    return;
                                }
                            }
                        }
                        MrtrAction::Abort(reason) => {
                            yield ToolCallEvent::Complete(Err(CallToolError::Aborted {
                                server: server_name,
                                reason,
                                timeout: options.timeout,
                            }));
                            return;
                        }
                    }
                }
                Ok(CallToolResponse::Task(task)) => {
                    let driver = TaskDriver::new(&server_name, client.as_ref(), options.timeout, options.cancel.clone());
                    let mut events = Box::pin(driver.stream(task, progress));
                    while let Some(event) = events.next().await {
                        yield event;
                    }
                    return;
                }
                Ok(_) => {
                    yield ToolCallEvent::Complete(Err(CallToolError::UnsupportedResponse { server: server_name }));
                    return;
                }
                Err(e) => {
                    yield ToolCallEvent::Complete(Err(CallToolError::Call(e)));
                    return;
                }
            }
        }
    }
}

fn peer_request_options(options: &CallToolOptions) -> PeerRequestOptions {
    let request_options = PeerRequestOptions::with_timeout(options.timeout);
    match &options.meta {
        Some(meta) => request_options.with_meta(meta.clone()),
        None => request_options,
    }
}

async fn await_response_or_cancel(
    handle: RequestHandle<RoleClient>,
    cancel: &CancellationToken,
) -> Option<Result<CallToolResponse, ServiceError>> {
    let response = pin!(await_tool_response(handle));
    match select(response, pin!(cancel.cancelled())).await {
        Either::Left((response, _)) => Some(response),
        Either::Right(((), _)) => None,
    }
}

async fn await_tool_response(handle: RequestHandle<RoleClient>) -> Result<CallToolResponse, ServiceError> {
    match handle.await_response().await? {
        ServerResult::CallToolResult(result) => Ok(CallToolResponse::Complete(result)),
        ServerResult::InputRequiredResult(result) => Ok(CallToolResponse::InputRequired(result)),
        ServerResult::CreateTaskResult(result) => Ok(CallToolResponse::Task(result)),
        _ => Err(ServiceError::UnexpectedResponse),
    }
}

async fn elicit_input(
    client: &McpClient,
    mrtr_state: &mut MrtrState,
    server_name: &str,
    requests: InputRequests,
) -> Result<InputResponses, CallToolError> {
    let (responses, results) = elicit_inputs(client, requests).await.map_err(|error| match error {
        ElicitInputsError::UnsupportedInput => CallToolError::UnsupportedInput { server: server_name.to_string() },
        ElicitInputsError::Serialize(source) => {
            CallToolError::SerializeResponse { server: server_name.to_string(), source }
        }
    })?;
    for result in &results {
        mrtr_state.record_response(result);
    }
    Ok(responses)
}
