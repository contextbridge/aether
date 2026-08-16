use crate::events::TraceContext;
use futures::{Stream, StreamExt};
use llm::ToolCallRequest;
use mcp_utils::client::split_on_server_name;
use mcp_utils::client::{
    CallToolError, CallToolOptions, CancellationToken, McpClient, McpConnectAttempt, McpConnectionAttemptManager,
    McpError, McpManager, McpServer, ToolCallEvent, call_tool,
};
use mcp_utils::tool_gateway::ServerDescription;
use rmcp::{
    RoleClient,
    model::{CallToolRequestParams, CallToolResult, GetPromptResult, Prompt},
    service::RunningService,
};
use std::collections::HashSet;
use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::select;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinSet;

const MCP_AUTH_TIMEOUT: Duration = Duration::from_mins(3);

#[derive(Debug)]
pub(crate) enum McpCommand {
    ExecuteTool {
        request: ToolCallRequest,
        trace_context: Option<TraceContext>,
        timeout: Duration,
        tx: mpsc::Sender<ToolCallEvent>,
        cancel: CancellationToken,
    },
    ListDeferredServers {
        reply: oneshot::Sender<Vec<ServerDescription>>,
    },
    ListDeferredTools {
        reply: oneshot::Sender<Result<Vec<llm::ToolDefinition>, String>>,
    },
    ExecuteDeferredTool {
        request: CallToolRequestParams,
        reply: oneshot::Sender<Result<CallToolResult, String>>,
    },
    ListPrompts {
        tx: oneshot::Sender<Result<Vec<Prompt>, String>>,
    },
    GetPrompt {
        name: String,
        arguments: Option<serde_json::Map<String, serde_json::Value>>,
        tx: oneshot::Sender<Result<GetPromptResult, String>>,
    },
    AuthenticateServer {
        name: String,
    },
}

type ResolvedTool = Result<(Arc<RunningService<RoleClient, McpClient>>, CallToolRequestParams), String>;

pub(crate) async fn run_mcp_task(
    mut mcp: McpManager,
    mut command_rx: mpsc::Receiver<McpCommand>,
    pending_servers: Vec<McpServer>,
) {
    let mut background_operations = JoinSet::new();
    let mut manager_event_rx = mcp.take_event_receiver();
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
                on_command(command, &mut mcp, &mut mcp_connection_attempts, &mut background_operations).await;
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

            Some(event) = manager_event_rx.recv() => {
                mcp.apply_event(event).await;
            }

            Some(joined) = background_operations.join_next(), if !background_operations.is_empty() => {
                if let Err(e) = joined {
                    tracing::warn!("MCP background operation ended unexpectedly: {e:?}");
                }
            }
        }
    }

    background_operations.abort_all();
    while background_operations.join_next().await.is_some() {}
    mcp_connection_attempts.shutdown().await;
    mcp.shutdown().await;
    tracing::debug!("MCP manager task ended");
}

async fn on_command(
    command: McpCommand,
    mcp: &mut McpManager,
    auth_tasks: &mut McpConnectionAttemptManager,
    background_operations: &mut JoinSet<()>,
) {
    match command {
        McpCommand::ExecuteTool { request, trace_context, timeout, tx, cancel } => {
            let resolved = mcp
                .get_client_for_tool(&request.name, &request.arguments)
                .map_err(|error| format!("Failed to get client for tool '{}': {error}", request.name));
            let options = CallToolOptions {
                timeout,
                meta: trace_context.as_ref().map(TraceContext::to_meta),
                cancel: cancel.clone(),
            };
            spawn_streaming_tool_execution(resolved, options, tx, cancel, background_operations).await;
        }

        McpCommand::ListDeferredServers { reply } => {
            let _ = reply.send(mcp.deferred_servers());
        }

        McpCommand::ListDeferredTools { reply } => {
            let mut tools = Vec::new();
            let mut error = None;
            for server in mcp.deferred_servers() {
                match mcp.list_deferred_tools(&server.name).await {
                    Ok(server_tools) => tools.extend(server_tools.into_iter().map(|mut tool| {
                        tool.name = format!("{}__{}", server.name, tool.name);
                        tool
                    })),
                    Err(cause) => {
                        error = Some(cause.to_string());
                        break;
                    }
                }
            }
            let _ = reply.send(error.map_or(Ok(tools), Err));
        }

        McpCommand::ExecuteDeferredTool { request, reply } => {
            let Some((server, tool)) = split_on_server_name(&request.name) else {
                let _ = reply.send(Err(format!("invalid deferred tool name '{}'", request.name)));
                return;
            };
            let resolved = mcp
                .resolve_deferred_tool(server, tool, request.arguments.unwrap_or_default())
                .map_err(|error| error.to_string());
            match resolved {
                Ok((client, params)) => {
                    let cancel = CancellationToken::new();
                    let options = CallToolOptions {
                        timeout: Duration::from_mins(10),
                        meta: request.meta,
                        cancel: cancel.clone(),
                    };
                    background_operations.spawn(execute_gateway_tool(
                        call_tool(client, params, options),
                        reply,
                        cancel,
                    ));
                }
                Err(error) => {
                    let _ = reply.send(Err(error));
                }
            }
        }

        McpCommand::ListPrompts { tx } => {
            let task = mcp.list_prompts();
            background_operations.spawn(async move {
                let result = task.await.map_err(|e| format!("Failed to list prompts: {e}"));
                let _ = tx.send(result);
            });
        }

        McpCommand::GetPrompt { name: namespaced_name, arguments, tx } => {
            let task = mcp.get_prompt(&namespaced_name, arguments);
            background_operations.spawn(async move {
                let result = task.await.map_err(|e| format!("Failed to get prompt: {e}"));
                let _ = tx.send(result);
            });
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
                        ),
                    }
                });
            }
            Err(e) => tracing::warn!("Authentication failed for '{name}': {e}"),
        },
    }
}

async fn execute_gateway_tool(
    events: impl Stream<Item = ToolCallEvent>,
    mut reply: oneshot::Sender<Result<CallToolResult, String>>,
    cancel: CancellationToken,
) {
    let mut result = pin!(gateway_tool_result(events));
    select! {
        () = reply.closed() => {
            cancel.cancel();
            let _ = result.as_mut().await;
        },
        result = result.as_mut() => {
            let _ = reply.send(result);
        }
    }
}

async fn gateway_tool_result(events: impl Stream<Item = ToolCallEvent>) -> Result<CallToolResult, String> {
    let mut events = pin!(events);
    while let Some(event) = events.next().await {
        match event {
            ToolCallEvent::Complete(result) | ToolCallEvent::TaskComplete { result, .. } => {
                return result.map_err(|error| error.to_string());
            }
            ToolCallEvent::Cancelled { .. } => return Err("MCP tool call was cancelled".into()),
            ToolCallEvent::Progress(_) | ToolCallEvent::TaskCreated(_) | ToolCallEvent::TaskStatus(_) => {}
        }
    }
    Err("MCP manager is unavailable".into())
}

async fn execute_streaming_tool(
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

async fn spawn_streaming_tool_execution(
    resolved: ResolvedTool,
    options: CallToolOptions,
    tx: mpsc::Sender<ToolCallEvent>,
    cancel: CancellationToken,
    background_operations: &mut JoinSet<()>,
) {
    match resolved {
        Ok((client, params)) => {
            background_operations.spawn(execute_streaming_tool(call_tool(client, params, options), tx, cancel));
        }
        Err(message) => {
            tracing::error!("{message}");
            let error = CallToolError::Unavailable { message };
            let _ = tx.send(ToolCallEvent::Complete(Err(error))).await;
        }
    }
}
