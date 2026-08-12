use crate::events::TraceContext;
use mcp_utils::client::{
    CallToolError, CallToolOptions, CancellationToken, McpConnectAttempt, McpConnectionAttemptManager, McpError,
    McpManager, McpServer, McpServerStatusEntry, ToolCallEvent, call_tool,
};

use futures::{Stream, StreamExt};
use llm::ToolCallRequest;
use rmcp::model::{GetPromptResult, Prompt};
use std::collections::HashSet;
use std::pin::pin;
use std::time::Duration;
use tokio::select;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinSet;

const MCP_AUTH_TIMEOUT: Duration = Duration::from_mins(3);

/// Commands that can be sent to the MCP manager task
#[derive(Debug)]
pub enum McpCommand {
    ExecuteTool {
        request: ToolCallRequest,
        trace_context: Option<TraceContext>,
        timeout: Duration,
        tx: mpsc::Sender<ToolCallEvent>,
        cancel: CancellationToken,
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
    let mut tool_executions = JoinSet::new();
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
                on_command(command, &mut mcp, &mut mcp_connection_attempts, &mut tool_executions).await;
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

            Some(joined) = tool_executions.join_next(), if !tool_executions.is_empty() => {
                if let Err(e) = joined {
                    tracing::warn!("MCP tool execution ended unexpectedly: {e:?}");
                }
            }
        }
    }

    tool_executions.abort_all();
    while tool_executions.join_next().await.is_some() {}
    mcp_connection_attempts.shutdown().await;
    mcp.shutdown().await;
    tracing::debug!("MCP manager task ended");
}

async fn execute_tool(
    events: impl Stream<Item = ToolCallEvent>,
    tx: mpsc::Sender<ToolCallEvent>,
    cancel: CancellationToken,
) {
    let mut events = pin!(events);
    while let Some(event) = events.next().await {
        if tx.send(event).await.is_err() {
            cancel.cancel();
            break;
        }
    }
}

async fn on_command(
    command: McpCommand,
    mcp: &mut McpManager,
    auth_tasks: &mut McpConnectionAttemptManager,
    tool_executions: &mut JoinSet<()>,
) {
    match command {
        McpCommand::ExecuteTool { request, trace_context, timeout, tx, cancel } => {
            match mcp.get_client_for_tool(&request.name, &request.arguments) {
                Ok((client, params)) => {
                    let options = CallToolOptions {
                        timeout,
                        meta: trace_context.as_ref().map(TraceContext::to_meta),
                        cancel: cancel.clone(),
                    };
                    tool_executions.spawn(execute_tool(call_tool(client, params, options), tx, cancel));
                }
                Err(e) => {
                    tracing::error!("Failed to get client for tool {}: {e}", request.name);
                    let error = CallToolError::Unavailable { message: format!("Failed to get client: {e}") };
                    let _ = tx.send(ToolCallEvent::Complete(Err(error))).await;
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
