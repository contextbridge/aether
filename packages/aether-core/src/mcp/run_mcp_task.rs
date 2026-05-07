use mcp_utils::client::{ConnectedServer, ConnectionError, McpClient, McpError, McpManager, McpServerStatusEntry};
use mcp_utils::display_meta::ToolResultMeta;

use futures::future::Either;
use futures::stream::{self, StreamExt};
use llm::{ToolCallError, ToolCallRequest, ToolCallResult};
use rmcp::RoleClient;
use rmcp::model::{
    CallToolRequestParams, CreateElicitationRequestParams, ErrorCode, GetPromptResult, ProgressNotificationParam,
    Prompt,
};
use rmcp::service::RunningService;
use std::sync::Arc;
use std::time::Duration;
use tokio::select;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::task::JoinSet;

/// Events emitted during tool execution lifecycle
#[derive(Debug)]
pub enum ToolExecutionEvent {
    Started { tool_id: String, tool_name: String },
    Progress { tool_id: String, progress: ProgressNotificationParam },
    Complete { tool_id: String, result: Result<ToolCallResult, ToolCallError>, result_meta: Option<ToolResultMeta> },
}

const MCP_AUTH_TIMEOUT: Duration = Duration::from_mins(3);

/// Commands that can be sent to the MCP manager task
#[derive(Debug)]
pub enum McpCommand {
    ExecuteTool {
        request: ToolCallRequest,
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

pub async fn run_mcp_task(mut mcp: McpManager, mut command_rx: mpsc::Receiver<McpCommand>) {
    let mut auth_tasks = JoinSet::<(String, Result<ConnectedServer, ConnectionError>)>::new();
    loop {
        select! {
            command = command_rx.recv() => {
                let Some(command) = command else { break; };
                on_command(command, &mut mcp, &mut auth_tasks).await;
            }

            Some(joined) = auth_tasks.join_next(), if !auth_tasks.is_empty() => {
                match joined {
                    Ok((_name, attempt)) => {
                        mcp.apply_connection_attempt(attempt).await;
                    }
                    Err(e) => {
                        tracing::error!("MCP auth task did not complete normally: {e:?}");
                    }
                }
            }
        }
    }

    auth_tasks.abort_all();
    while auth_tasks.join_next().await.is_some() {}
    mcp.shutdown().await;
    tracing::debug!("MCP manager task ended");
}

async fn on_command(
    command: McpCommand,
    mcp: &mut McpManager,
    auth_tasks: &mut JoinSet<(String, Result<ConnectedServer, ConnectionError>)>,
) {
    match command {
        McpCommand::ExecuteTool { request, timeout, tx } => {
            let tool_id = request.id.clone();
            let tool_name = request.name.clone();

            let _ =
                tx.send(ToolExecutionEvent::Started { tool_id: tool_id.clone(), tool_name: tool_name.clone() }).await;

            match mcp.get_client_for_tool(&request.name, &request.arguments) {
                Ok((client, params)) => {
                    tokio::spawn(async move {
                        let outcome =
                            execute_mcp_call(client, &request, params, timeout, tool_id.clone(), tx.clone()).await;
                        let (result, result_meta) = match outcome {
                            Ok((r, m)) => (Ok(r), m),
                            Err(e) => (Err(e), None),
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
            let _ = tx.send(mcp.server_statuses().to_vec());
        }

        McpCommand::AuthenticateServer { name } => match mcp.authenticate_server_task(&name).await {
            Ok(task) => {
                let server_name = name.clone();
                auth_tasks.spawn(async move {
                    let attempt = match tokio::time::timeout(MCP_AUTH_TIMEOUT, task).await {
                        Ok(attempt) => attempt,
                        Err(_) => Err(ConnectionError::Failed {
                            name: server_name.clone(),
                            error: McpError::ConnectionFailed("authentication timed out after 3 minutes".to_string()),
                            proxy: false,
                        }),
                    };
                    (server_name, attempt)
                });
            }
            Err(e) => tracing::warn!("Authentication failed for '{name}': {e}"),
        },
    }
}

/// Shared logic for sending an MCP tool call, streaming progress events,
/// and collecting the result.
async fn execute_mcp_call(
    client: Arc<RunningService<RoleClient, McpClient>>,
    request: &ToolCallRequest,
    params: CallToolRequestParams,
    timeout: Duration,
    tool_call_id: String,
    event_tx: mpsc::Sender<ToolExecutionEvent>,
) -> Result<(ToolCallResult, Option<ToolResultMeta>), ToolCallError> {
    use super::tool_bridge::mcp_result_to_tool_call_result;
    use rmcp::model::{ClientRequest::CallToolRequest, Request, ServerResult};
    use rmcp::service::PeerRequestOptions;

    let handle = client
        .send_cancellable_request(CallToolRequest(Request::new(params)), {
            let mut opts = PeerRequestOptions::default();
            opts.timeout = Some(timeout);
            opts
        })
        .await
        .map_err(|e| ToolCallError::from_request(request, format!("Failed to send tool request: {e}")))?;

    let progress_subscriber = client.service().progress_dispatcher.subscribe(handle.progress_token.clone()).await;

    let progress_stream = progress_subscriber
        .map(move |progress| Either::Left(ToolExecutionEvent::Progress { tool_id: tool_call_id.clone(), progress }));

    let result_stream = stream::once(handle.await_response()).map(Either::Right);
    let combined_stream = stream::select(progress_stream, result_stream);
    tokio::pin!(combined_stream);

    let server_result = loop {
        match combined_stream.next().await {
            Some(Either::Left(progress_event)) => {
                let _ = event_tx.send(progress_event).await;
            }
            Some(Either::Right(result)) => {
                break match result {
                    Ok(server_result) => server_result,
                    Err(e) => {
                        if let rmcp::service::ServiceError::McpError(ref error_data) = e
                            && error_data.code == ErrorCode::URL_ELICITATION_REQUIRED
                        {
                            return Err(handle_url_elicitation_required(&client, request, error_data).await);
                        }
                        return Err(ToolCallError::from_request(request, format!("Tool execution failed: {e}")));
                    }
                };
            }
            None => {
                return Err(ToolCallError::from_request(request, "Stream ended without result"));
            }
        }
    };

    let ServerResult::CallToolResult(mcp_result) = server_result else {
        return Err(ToolCallError::from_request(request, "Unexpected response type from MCP server"));
    };

    mcp_result_to_tool_call_result(request, mcp_result)
}

#[derive(serde::Deserialize)]
struct UrlElicitationRequiredData {
    elicitations: Vec<CreateElicitationRequestParams>,
}

#[derive(Debug)]
enum UrlElicitationRequiredParseError {
    MissingData,
    InvalidData(serde_json::Error),
    NoUrlRequests,
}

impl std::fmt::Display for UrlElicitationRequiredParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingData => write!(f, "missing error data"),
            Self::InvalidData(error) => write!(f, "malformed error data: {error}"),
            Self::NoUrlRequests => write!(f, "provided no URL elicitation requests"),
        }
    }
}

fn parse_required_url_elicitations(
    error_data: &rmcp::model::ErrorData,
) -> Result<Vec<CreateElicitationRequestParams>, UrlElicitationRequiredParseError> {
    let data = error_data.data.as_ref().ok_or(UrlElicitationRequiredParseError::MissingData)?;
    let parsed: UrlElicitationRequiredData =
        serde_json::from_value(data.clone()).map_err(UrlElicitationRequiredParseError::InvalidData)?;

    let url_elicitations = parsed
        .elicitations
        .into_iter()
        .filter(|elicitation| matches!(elicitation, CreateElicitationRequestParams::UrlElicitationParams { .. }))
        .collect::<Vec<_>>();

    if url_elicitations.is_empty() {
        return Err(UrlElicitationRequiredParseError::NoUrlRequests);
    }

    Ok(url_elicitations)
}

/// Handle a `URL_ELICITATION_REQUIRED` (-32042) error by dispatching each
/// URL elicitation through the same consent channel used by normal
/// `create_elicitation` requests.
async fn handle_url_elicitation_required(
    client: &Arc<RunningService<RoleClient, McpClient>>,
    request: &ToolCallRequest,
    error_data: &rmcp::model::ErrorData,
) -> ToolCallError {
    let server_name = client.service().server_name().to_string();
    let url_elicitations = match parse_required_url_elicitations(error_data) {
        Ok(url_elicitations) => url_elicitations,
        Err(UrlElicitationRequiredParseError::NoUrlRequests) => {
            return ToolCallError::from_request(
                request,
                format!("Server '{server_name}' requires URL elicitation but provided no URL elicitation requests"),
            );
        }
        Err(parse_error) => {
            return ToolCallError::from_request(
                request,
                format!("Server '{server_name}' sent an invalid URL elicitation response: {parse_error}"),
            );
        }
    };

    tracing::info!("Server '{server_name}' requires {} URL elicitation(s)", url_elicitations.len());

    for elicitation in url_elicitations {
        let result = client.service().dispatch_elicitation(elicitation).await;
        match result.action {
            rmcp::model::ElicitationAction::Decline => {
                return ToolCallError::from_request(
                    request,
                    format!("Required browser interaction for server '{server_name}' was declined"),
                );
            }
            rmcp::model::ElicitationAction::Cancel => {
                return ToolCallError::from_request(
                    request,
                    format!("Required browser interaction for server '{server_name}' was cancelled"),
                );
            }
            rmcp::model::ElicitationAction::Accept => {
                tracing::info!("User accepted URL elicitation for server '{server_name}'");
            }
        }
    }

    ToolCallError::from_request(
        request,
        format!(
            "Server '{server_name}' requires a browser flow. The URL has been opened for your approval. Retry the previous request after completing the browser flow."
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_elicitation_required_data_parses_url_entries() {
        let data = serde_json::json!({
            "elicitations": [
                {
                    "mode": "url",
                    "message": "Auth",
                    "url": "https://example.com/auth?elicitationId=el-1",
                    "elicitationId": "el-1"
                }
            ]
        });

        let parsed: UrlElicitationRequiredData = serde_json::from_value(data).unwrap();
        assert_eq!(parsed.elicitations.len(), 1);
        assert!(matches!(
            &parsed.elicitations[0],
            CreateElicitationRequestParams::UrlElicitationParams { elicitation_id, .. } if elicitation_id == "el-1"
        ));
    }

    #[test]
    fn parse_required_url_elicitations_filters_to_url_only() {
        let error_data = rmcp::model::ErrorData {
            code: rmcp::model::ErrorCode::URL_ELICITATION_REQUIRED,
            message: "URL elicitation required".into(),
            data: Some(serde_json::json!({
                "elicitations": [
                    {
                        "mode": "url",
                        "message": "Auth",
                        "url": "https://example.com/auth",
                        "elicitationId": "el-1"
                    },
                    {
                        "mode": "form",
                        "message": "Pick a color",
                        "requestedSchema": { "type": "object", "properties": {} }
                    }
                ]
            })),
        };

        let result = parse_required_url_elicitations(&error_data).unwrap();
        assert_eq!(result.len(), 1);
        assert!(matches!(
            &result[0],
            CreateElicitationRequestParams::UrlElicitationParams { elicitation_id, .. } if elicitation_id == "el-1"
        ));
    }
}
