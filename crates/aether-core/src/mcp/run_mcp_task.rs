use crate::events::TraceContext;
use mcp_utils::client::{
    McpClient, McpConnectAttempt, McpConnectionAttemptManager, McpError, McpManager, McpServer, McpServerStatusEntry,
};
use mcp_utils::display_meta::ToolResultMeta;

use futures::future::Either;
use futures::stream::StreamExt;
use llm::{ToolCallError, ToolCallRequest, ToolCallResult};
use rmcp::RoleClient;
use rmcp::model::{
    CallToolRequestParams, ElicitRequestParams, ElicitResult, ElicitationAction, GetPromptResult, InputRequest,
    InputRequiredResult, InputResponses, ProgressNotificationParam, Prompt, RequestMetaObject, ServerResult,
};
use rmcp::service::{PeerRequestOptions, RequestHandle, RunningService};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::select;
use tokio::sync::mpsc;
use tokio::sync::oneshot;

/// Events emitted during tool execution lifecycle
#[derive(Debug)]
pub enum ToolExecutionEvent {
    Progress { tool_id: String, progress: ProgressNotificationParam },
    Complete { tool_id: String, result: Result<ToolCallResult, ToolCallError>, result_meta: Option<ToolResultMeta> },
}

/// Maximum number of `input_required` rounds the MRTR executor resolves before
/// failing. After exactly this many input rounds, one final retry is still
/// attempted; a further `input_required` fails the operation before its input
/// is resolved.
pub const MRTR_MAX_ROUNDS: usize = 8;

/// Errors raised by the MRTR-aware tool execution loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolExecutionError {
    /// The server replied with a result shape the executor cannot drive, the
    /// request could not be sent, or the response stream ended unexpectedly.
    ProtocolMismatch { detail: String },
    /// The server kept returning `input_required` beyond the bounded input-round limit.
    ExcessiveRounds { max_rounds: usize },
    /// The server asked for input Aether cannot provide, or a collected
    /// response could not be represented.
    InvalidInputResponse { detail: String },
    /// The whole operation exceeded its single deadline, including input waits.
    Timeout { timeout: Duration },
    /// The operation was cancelled: the consumer dropped the tool execution
    /// event stream, or the server cancelled a request.
    Cancelled { reason: Option<String> },
    /// The tool itself failed (an `isError` result); surfaced verbatim.
    ToolError(ToolCallError),
}

impl std::fmt::Display for ToolExecutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ProtocolMismatch { detail } => write!(f, "MCP protocol mismatch: {detail}"),
            Self::ExcessiveRounds { max_rounds } => {
                write!(f, "server kept requesting input beyond {max_rounds} MRTR input rounds")
            }
            Self::InvalidInputResponse { detail } => write!(f, "invalid MRTR input response: {detail}"),
            Self::Timeout { timeout } => write!(f, "tool execution timed out after {timeout:?}"),
            Self::Cancelled { reason: Some(reason) } => write!(f, "tool execution was cancelled: {reason}"),
            Self::Cancelled { reason: None } => write!(f, "tool execution was cancelled"),
            Self::ToolError(error) => write!(f, "{error:?}"),
        }
    }
}

impl std::error::Error for ToolExecutionError {}

const MCP_AUTH_TIMEOUT: Duration = Duration::from_mins(3);

/// Commands that can be sent to the MCP manager task
#[derive(Debug)]
pub enum McpCommand {
    ExecuteTool {
        request: ToolCallRequest,
        trace_context: Option<TraceContext>,
        timeout: Duration,
        tx: mpsc::Sender<ToolExecutionEvent>,
    },
    ListPrompts {
        tx: oneshot::Sender<Result<Vec<Prompt>, String>>,
    },
    GetPrompt {
        name: String,
        arguments: Option<serde_json::Map<String, serde_json::Value>>,
        tx: oneshot::Sender<Result<GetPromptResult, String>>,
    },
    GetServerStatuses {
        tx: oneshot::Sender<Vec<McpServerStatusEntry>>,
    },
    AuthenticateServer {
        name: String,
    },
}

pub async fn run_mcp_task(
    mut mcp: McpManager,
    mut command_rx: mpsc::Receiver<McpCommand>,
    pending_servers: Vec<McpServer>,
) {
    let mut mcp_connection_attempts = McpConnectionAttemptManager::default();
    let mut pending_connections: HashSet<String> = pending_servers.iter().map(|server| server.name.clone()).collect();
    for server in pending_servers {
        let name = server.name.clone();
        let task = mcp.connect_pending_task(server);
        mcp_connection_attempts.spawn(name, task);
    }
    if pending_connections.is_empty() {
        mcp.emit_connection_ready().await;
    }

    loop {
        select! {
            command = command_rx.recv() => {
                let Some(command) = command else { break; };
                on_command(command, &mut mcp, &mut mcp_connection_attempts).await;
            }

            Some(joined) = mcp_connection_attempts.join_next(), if !mcp_connection_attempts.is_empty() => {
                match joined {
                    Ok(attempt) => {
                        let was_bootstrap = pending_connections.remove(&attempt.name);
                        mcp.apply_connection_attempt(attempt).await;
                        if was_bootstrap && pending_connections.is_empty() {
                            mcp.emit_connection_ready().await;
                        }
                    }
                    Err(e) => tracing::error!("MCP auth task did not complete normally: {e:?}"),
                }
            }
        }
    }

    mcp_connection_attempts.shutdown().await;
    mcp.shutdown().await;
    tracing::debug!("MCP manager task ended");
}

async fn on_command(command: McpCommand, mcp: &mut McpManager, auth_tasks: &mut McpConnectionAttemptManager) {
    match command {
        McpCommand::ExecuteTool { request, trace_context, timeout, tx } => {
            let tool_id = request.id.clone();

            match mcp.get_client_for_tool(&request.name, &request.arguments) {
                Ok((client, params)) => {
                    let trace_meta = trace_context.as_ref().map(TraceContext::to_meta);
                    tokio::spawn(async move {
                        let outcome = execute_mcp_call(
                            client,
                            &request,
                            params,
                            trace_meta,
                            timeout,
                            tool_id.clone(),
                            tx.clone(),
                        )
                        .await;
                        let (result, result_meta) = match outcome {
                            Ok((r, m)) => (Ok(r), m),
                            Err(ToolExecutionError::ToolError(e)) => (Err(e), None),
                            Err(e) => (Err(ToolCallError::from_request(&request, e.to_string())), None),
                        };
                        let _ = tx.send(ToolExecutionEvent::Complete { tool_id, result, result_meta }).await;
                    });
                }
                Err(e) => {
                    tracing::error!("Failed to get client for tool {}: {e}", request.name);
                    let error = ToolCallError::from_request(&request, format!("Failed to get client: {e}"));
                    let _ =
                        tx.send(ToolExecutionEvent::Complete { tool_id, result: Err(error), result_meta: None }).await;
                }
            }
        }

        McpCommand::ListPrompts { tx } => {
            let result = mcp.list_prompts().await.map_err(|e| format!("Failed to list prompts: {e}"));
            let _ = tx.send(result);
        }

        McpCommand::GetPrompt { name: namespaced_name, arguments, tx } => {
            let result =
                mcp.get_prompt(&namespaced_name, arguments).await.map_err(|e| format!("Failed to get prompt: {e}"));
            let _ = tx.send(result);
        }

        McpCommand::GetServerStatuses { tx } => {
            let _ = tx.send(mcp.server_statuses());
        }

        McpCommand::AuthenticateServer { name } => match mcp.authenticate_server_task(&name).await {
            Ok(task) => {
                let server_name = name.clone();
                auth_tasks.spawn(name, async move {
                    match tokio::time::timeout(MCP_AUTH_TIMEOUT, task).await {
                        Ok(attempt) => attempt,
                        Err(_) => McpConnectAttempt::failed(
                            server_name,
                            McpError::ConnectionFailed("authentication timed out after 3 minutes".to_string()),
                            false,
                        ),
                    }
                });
            }
            Err(e) => tracing::warn!("Authentication failed for '{name}': {e}"),
        },
    }
}

/// MRTR-aware tool execution: send one cancellable `tools/call` per round,
/// resolving any `InputRequiredResult` through the existing client
/// handler/UI elicitation channel and retrying with the collected
/// `inputResponses` and the exact opaque `requestState`, until a final
/// `CallToolResult`, an error, or the bounded input-round limit.
///
/// One deadline bounds the whole operation: every round's request and every
/// input wait count against the same clock, and progress events stream across
/// rounds on `event_tx`. The consumer's `ToolExecutionEvent` receiver is the
/// cancellation signal: dropping it cancels the operation both while a request
/// is in flight and while the elicitation UI is open. After `MRTR_MAX_ROUNDS`
/// input rounds one final retry is still attempted; a further `input_required`
/// fails the operation before its input is resolved.
async fn execute_mcp_call(
    client: Arc<RunningService<RoleClient, McpClient>>,
    request: &ToolCallRequest,
    params: CallToolRequestParams,
    trace_meta: Option<RequestMetaObject>,
    timeout: Duration,
    tool_call_id: String,
    event_tx: mpsc::Sender<ToolExecutionEvent>,
) -> Result<(ToolCallResult, Option<ToolResultMeta>), ToolExecutionError> {
    use super::tool_bridge::mcp_result_to_tool_call_result;

    let deadline = tokio::time::Instant::now().checked_add(timeout);
    let mut collected_responses: Option<InputResponses> = None;
    let mut request_state: Option<String> = None;
    let mut input_rounds = 0usize;

    loop {
        remaining_before(deadline, timeout)?;
        let round_params = build_round_params(&params, collected_responses.as_ref(), request_state.as_deref());
        let server_result =
            send_mrtr_round(&client, round_params, trace_meta.clone(), deadline, timeout, &tool_call_id, &event_tx)
                .await?;

        match server_result {
            ServerResult::CallToolResult(mcp_result) => {
                return mcp_result_to_tool_call_result(request, mcp_result).map_err(ToolExecutionError::ToolError);
            }
            ServerResult::InputRequiredResult(input_required) => {
                if input_rounds >= MRTR_MAX_ROUNDS {
                    return Err(ToolExecutionError::ExcessiveRounds { max_rounds: MRTR_MAX_ROUNDS });
                }
                input_rounds += 1;
                let state = input_required.request_state.clone();
                let responses = collect_input_responses(&client, input_required, deadline, timeout, &event_tx).await?;
                if !responses.is_empty() {
                    collected_responses = Some(match collected_responses {
                        Some(mut all) => {
                            all.extend(responses);
                            all
                        }
                        None => responses,
                    });
                }
                request_state = state;
            }
            other => {
                return Err(ToolExecutionError::ProtocolMismatch {
                    detail: format!("unexpected result type from server: {other:?}"),
                });
            }
        }
    }
}

/// Send one cancellable `tools/call` request, streaming progress events and
/// returning the raw `ServerResult`. The operation deadline is enforced here,
/// and the consumer's event receiver lifetime acts as the cancellation signal:
/// dropping it cancels the in-flight request and aborts the round.
#[allow(clippy::too_many_arguments)]
async fn send_mrtr_round(
    client: &Arc<RunningService<RoleClient, McpClient>>,
    params: CallToolRequestParams,
    trace_meta: Option<RequestMetaObject>,
    deadline: Option<tokio::time::Instant>,
    timeout: Duration,
    tool_call_id: &str,
    event_tx: &mpsc::Sender<ToolExecutionEvent>,
) -> Result<ServerResult, ToolExecutionError> {
    use rmcp::model::{ClientRequest::CallToolRequest, Request};

    let mut handle = Some(
        client
            .send_cancellable_request(CallToolRequest(Request::new(params)), {
                let mut opts = PeerRequestOptions::default();
                opts.meta = trace_meta;
                opts
            })
            .await
            .map_err(|e| ToolExecutionError::ProtocolMismatch {
                detail: format!("Failed to send tool request: {e}"),
            })?,
    );

    let progress_token = handle.as_ref().expect("round handle").progress_token.clone();
    let mut progress_stream = client.service().progress_dispatcher.subscribe(progress_token).await;
    let mut progress_open = true;

    loop {
        let step = tokio::select! {
            biased;
            progress = progress_stream.next(), if progress_open => Either::Left(progress),
            outcome = await_round_response(
                handle.as_mut().expect("round handle"),
                deadline,
                timeout,
                event_tx,
            ) => Either::Right(outcome),
        };

        match step {
            Either::Left(Some(progress)) => {
                let event = ToolExecutionEvent::Progress { tool_id: tool_call_id.to_string(), progress };
                if event_tx.send(event).await.is_err() {
                    let reason = "consumer dropped the tool execution event stream";
                    return cancel_round(
                        handle,
                        reason,
                        ToolExecutionError::Cancelled { reason: Some(reason.to_string()) },
                    )
                    .await;
                }
            }
            Either::Left(None) => progress_open = false,
            Either::Right(RoundOutcome::Response(result)) => return result,
            Either::Right(RoundOutcome::ConsumerCancelled) => {
                let reason = "consumer dropped the tool execution event stream";
                return cancel_round(
                    handle,
                    reason,
                    ToolExecutionError::Cancelled { reason: Some(reason.to_string()) },
                )
                .await;
            }
            Either::Right(RoundOutcome::DeadlineExceeded) => {
                return cancel_round(
                    handle,
                    RequestHandle::<RoleClient>::REQUEST_TIMEOUT_REASON,
                    ToolExecutionError::Timeout { timeout },
                )
                .await;
            }
        }
    }
}

/// How waiting for one round ended.
#[allow(clippy::large_enum_variant)]
enum RoundOutcome {
    Response(Result<ServerResult, ToolExecutionError>),
    ConsumerCancelled,
    DeadlineExceeded,
}

/// Wait for the round's `ServerResult`, enforcing the operation deadline, and
/// observe the consumer's event receiver so a dropped receiver cancels the
/// whole operation rather than just the current request.
async fn await_round_response(
    handle: &mut RequestHandle<RoleClient>,
    deadline: Option<tokio::time::Instant>,
    timeout: Duration,
    event_tx: &mpsc::Sender<ToolExecutionEvent>,
) -> RoundOutcome {
    use rmcp::service::ServiceError;

    tokio::select! {
        biased;
        response = async {
            let response = match deadline {
                Some(deadline) => match tokio::time::timeout_at(deadline, &mut handle.rx).await {
                    Ok(response) => response,
                    Err(_) => return RoundOutcome::DeadlineExceeded,
                },
                None => (&mut handle.rx).await,
            };
            match response {
                Ok(Ok(server_result)) => RoundOutcome::Response(Ok(server_result)),
                Ok(Err(ServiceError::TransportClosed)) | Err(_) => {
                    RoundOutcome::Response(Err(ToolExecutionError::ProtocolMismatch {
                        detail: "response stream ended without a result".into(),
                    }))
                }
                Ok(Err(error)) => RoundOutcome::Response(Err(map_round_error(error, timeout))),
            }
        } => response,
        () = event_tx.closed() => RoundOutcome::ConsumerCancelled,
    }
}

/// Cancel the in-flight round request with `reason`, then return `error`.
async fn cancel_round(
    handle: Option<RequestHandle<RoleClient>>,
    reason: &str,
    error: ToolExecutionError,
) -> Result<ServerResult, ToolExecutionError> {
    if let Some(handle) = handle {
        let _ = handle.cancel(Some(reason.to_string())).await;
    }
    Err(error)
}

/// Resolve an `InputRequiredResult` into the responses to forward on retry.
/// The whole input wait counts against the operation deadline, and a dropped
/// consumer event receiver cancels the wait.
async fn collect_input_responses(
    client: &Arc<RunningService<RoleClient, McpClient>>,
    input_required: InputRequiredResult,
    deadline: Option<tokio::time::Instant>,
    timeout: Duration,
    event_tx: &mpsc::Sender<ToolExecutionEvent>,
) -> Result<InputResponses, ToolExecutionError> {
    let Some(requests) = input_required.input_requests else {
        if input_required.request_state.is_none() {
            return Err(ToolExecutionError::ProtocolMismatch {
                detail: "InputRequiredResult carried neither inputRequests nor requestState".into(),
            });
        }
        return Ok(InputResponses::new());
    };

    let mut responses = InputResponses::new();
    for (key, request) in requests {
        let response = match deadline {
            Some(deadline) => tokio::time::timeout_at(deadline, fulfill_input_request(client, request, event_tx))
                .await
                .map_err(|_| ToolExecutionError::Timeout { timeout })??,
            None => fulfill_input_request(client, request, event_tx).await?,
        };
        responses.insert(key, response);
    }
    Ok(responses)
}

/// Fulfill one server-initiated input request through the existing
/// `ClientHandler`/UI elicitation channel. The resulting `ElicitResult` is
/// forwarded verbatim, including decline and cancel actions, so the server
/// decides the final outcome. A dropped consumer event receiver cancels the
/// wait.
async fn fulfill_input_request(
    client: &Arc<RunningService<RoleClient, McpClient>>,
    request: InputRequest,
    event_tx: &mpsc::Sender<ToolExecutionEvent>,
) -> Result<serde_json::Value, ToolExecutionError> {
    let InputRequest::Elicitation(elicitation) = request else {
        return Err(ToolExecutionError::InvalidInputResponse {
            detail: "Aether cannot fulfill this input request: only elicitation/create is supported".into(),
        });
    };
    let is_form = matches!(&elicitation.params, ElicitRequestParams::FormElicitationParams { .. });

    let dispatch = client.service().dispatch_elicitation(elicitation.params);
    tokio::pin!(dispatch);
    let result = tokio::select! {
        biased;
        result = &mut dispatch => result,
        () = event_tx.closed() => {
            return Err(ToolExecutionError::Cancelled {
                reason: Some("consumer dropped the tool execution event stream".into()),
            });
        }
    };

    validate_elicitation_result(is_form, &result)?;
    serde_json::to_value(&result).map_err(|e| ToolExecutionError::InvalidInputResponse {
        detail: format!("failed to serialize elicitation result: {e}"),
    })
}

/// Accepted form responses must carry object-shaped `content` so the retry is
/// representable on the wire; decline and cancel are forwarded verbatim, and
/// URL acceptances carry no content. Full schema validation is deliberately
/// out of scope here (Phase 5).
fn validate_elicitation_result(is_form: bool, result: &ElicitResult) -> Result<(), ToolExecutionError> {
    if is_form
        && result.action == ElicitationAction::Accept
        && !matches!(result.content, Some(serde_json::Value::Object(_)))
    {
        return Err(ToolExecutionError::InvalidInputResponse {
            detail: "accepted form response must provide object-shaped content".into(),
        });
    }
    Ok(())
}

fn build_round_params(
    params: &CallToolRequestParams,
    input_responses: Option<&InputResponses>,
    request_state: Option<&str>,
) -> CallToolRequestParams {
    let mut round_params = params.clone();
    if let Some(responses) = input_responses {
        round_params = round_params.with_input_responses(responses.clone());
    }
    if let Some(state) = request_state {
        round_params = round_params.with_request_state(state.to_string());
    }
    round_params
}

fn remaining_before(
    deadline: Option<tokio::time::Instant>,
    timeout: Duration,
) -> Result<Option<Duration>, ToolExecutionError> {
    let Some(deadline) = deadline else { return Ok(None) };
    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    if remaining.is_zero() {
        return Err(ToolExecutionError::Timeout { timeout });
    }
    Ok(Some(remaining))
}

fn map_round_error(error: rmcp::service::ServiceError, timeout: Duration) -> ToolExecutionError {
    use rmcp::service::ServiceError;
    match error {
        ServiceError::Timeout { .. } => ToolExecutionError::Timeout { timeout },
        ServiceError::Cancelled { reason } => ToolExecutionError::Cancelled { reason },
        other => ToolExecutionError::ProtocolMismatch { detail: format!("tool execution failed: {other}") },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_execution_error_display_mentions_each_variant() {
        assert_eq!(
            ToolExecutionError::ProtocolMismatch { detail: "boom".into() }.to_string(),
            "MCP protocol mismatch: boom"
        );
        assert_eq!(
            ToolExecutionError::ExcessiveRounds { max_rounds: 8 }.to_string(),
            "server kept requesting input beyond 8 MRTR input rounds"
        );
        assert_eq!(
            ToolExecutionError::InvalidInputResponse { detail: "nope".into() }.to_string(),
            "invalid MRTR input response: nope"
        );
        assert_eq!(
            ToolExecutionError::Timeout { timeout: Duration::from_millis(300) }.to_string(),
            "tool execution timed out after 300ms"
        );
        assert_eq!(
            ToolExecutionError::Cancelled { reason: Some("stop".into()) }.to_string(),
            "tool execution was cancelled: stop"
        );
        assert_eq!(ToolExecutionError::Cancelled { reason: None }.to_string(), "tool execution was cancelled");
    }
}
