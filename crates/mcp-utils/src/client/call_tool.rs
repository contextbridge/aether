use crate::client::McpClient;
use crate::client::mrtr::{AbortReason, MrtrAction, MrtrState};
use async_stream::stream;
use futures::Stream;
use futures::StreamExt;
use futures::future::{Either, select};
use rmcp::RoleClient;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ClientRequest, InputRequest, InputRequests,
    InputResponses, ProgressNotificationParam, Request, RequestMetaObject, ServerResult,
};
use rmcp::service::{PeerRequestOptions, RequestHandle, RunningService, ServiceError};
use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Default)]
pub struct CallToolOptions {
    pub timeout: Duration,
    pub meta: Option<RequestMetaObject>,
}

#[derive(Debug)]
pub enum ToolCallEvent {
    Progress(ProgressNotificationParam),
    Complete(Result<CallToolResult, CallToolError>),
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
    #[error("Server '{server}' returned a task, but this client does not support the tasks extension")]
    UnsupportedTask { server: String },
    #[error("Server '{server}' returned a tool call response kind this client does not support")]
    UnsupportedResponse { server: String },
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
            let request_options = {
                let request_options = PeerRequestOptions::with_timeout(options.timeout);
                match &options.meta {
                    Some(meta) => request_options.with_meta(meta.clone()),
                    None => request_options,
                }
            };

            let handle = match client.send_cancellable_request(request, request_options).await {
                Ok(handle) => handle,
                Err(e) => {
                    yield ToolCallEvent::Complete(Err(CallToolError::Send(e)));
                    return;
                }
            };

            let mut progress = client.service().progress_dispatcher.subscribe(handle.progress_token.clone()).await;
            let mut response_future = pin!(await_tool_response(handle));
            let response = loop {
                match select(progress.next(), response_future.as_mut()).await {
                    Either::Left((Some(progress), _)) => yield ToolCallEvent::Progress(progress),
                    Either::Left((None, result_future)) => break result_future.await,
                    Either::Right((result, _)) => break result,
                }
            };

            match response {
                Ok(CallToolResponse::Complete(result)) => {
                    yield ToolCallEvent::Complete(Ok(result));
                    return;
                }
                Ok(CallToolResponse::InputRequired(input_required)) => {
                    match mrtr_state.tick(input_required) {
                        MrtrAction::Poll { backoff, request_state } => {
                            tokio::time::sleep(backoff).await;
                            params.input_responses = None;
                            params.request_state = Some(request_state);
                        }
                        MrtrAction::Elicit { input_requests, request_state } => {
                            let result = elicit_input(client.service(), &mut mrtr_state, &server_name, input_requests).await;
                            match result {
                                Ok(responses) => {
                                    params.input_responses = Some(responses);
                                    params.request_state = request_state;
                                }
                                Err(e) => {
                                    yield ToolCallEvent::Complete(Err(e));
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
                Ok(CallToolResponse::Task(_)) => {
                    yield ToolCallEvent::Complete(Err(CallToolError::UnsupportedTask { server: server_name }));
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
    let mut responses = InputResponses::new();
    for (key, request) in requests {
        let InputRequest::Elicitation(elicitation_request) = request else {
            return Err(CallToolError::UnsupportedInput { server: server_name.to_string() });
        };
        let result = client.dispatch_elicitation(elicitation_request.params).await;
        mrtr_state.record_response(&result);
        let response = serde_json::to_value(result)
            .map_err(|source| CallToolError::SerializeResponse { server: server_name.to_string(), source })?;
        responses.insert(key, response);
    }
    Ok(responses)
}
