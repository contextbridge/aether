use crate::events::TraceContext;
use crate::mcp::tool_bridge::mcp_result_to_tool_call_result;
use mcp_utils::client::{
    CallToolOptions, McpConnectAttempt, McpConnectionAttemptManager, McpError, McpManager, McpServer,
    McpServerStatusEntry, ToolCallEvent, call_tool,
};
use mcp_utils::display_meta::ToolResultMeta;

use futures::StreamExt;
use llm::{ToolCallError, ToolCallRequest, ToolCallResult};
use rmcp::model::{GetPromptResult, ProgressNotificationParam, Prompt};
use std::collections::HashSet;
use std::pin::pin;
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
                    let options = CallToolOptions { timeout, meta: trace_context.as_ref().map(TraceContext::to_meta) };
                    tokio::spawn(async move {
                        let mut events = pin!(call_tool(client, params, options));
                        while let Some(event) = events.next().await {
                            match event {
                                ToolCallEvent::Progress(progress) => {
                                    let progress_event =
                                        ToolExecutionEvent::Progress { tool_id: tool_id.clone(), progress };
                                    let _ = tx.send(progress_event).await;
                                }
                                ToolCallEvent::Complete(outcome) => {
                                    let (result, result_meta) = match outcome
                                        .map_err(|e| ToolCallError::from_request(&request, e.to_string()))
                                        .and_then(|mcp_result| mcp_result_to_tool_call_result(&request, mcp_result))
                                    {
                                        Ok((result, meta)) => (Ok(result), meta),
                                        Err(e) => (Err(e), None),
                                    };
                                    let _ =
                                        tx.send(ToolExecutionEvent::Complete { tool_id, result, result_meta }).await;
                                    return;
                                }
                            }
                        }
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
