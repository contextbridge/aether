use super::{
    connection_attempt_manager::McpConnectionAttemptManager,
    manager::{McpManager, RuntimeMcpServer},
};
use std::collections::HashSet;
use tokio::select;
use tokio::task::JoinSet;

#[cfg(feature = "oauth")]
use super::{McpError, connection::McpConnectAttempt};
#[cfg(feature = "oauth")]
use std::time::Duration;
#[cfg(feature = "oauth")]
use tokio::sync::oneshot;

#[cfg(feature = "oauth")]
const MCP_AUTH_TIMEOUT: Duration = Duration::from_mins(3);

#[derive(Debug)]
pub(crate) enum ManagerCommand {
    #[cfg(feature = "oauth")]
    AuthenticateServer { name: String, tx: oneshot::Sender<Result<(), McpError>> },
}

pub(super) async fn run(mut mcp: McpManager, pending_servers: Vec<RuntimeMcpServer>) {
    let mut command_rx = mcp.take_command_receiver();
    let mut tool_refresh_rx = mcp.take_tool_refresh_receiver();
    let snapshots = mcp.snapshot_sender();
    let mut tool_refreshes = JoinSet::new();
    let mut attempts = McpConnectionAttemptManager::default();
    let mut pending_connections: HashSet<String> = pending_servers.iter().map(|server| server.name.clone()).collect();
    for server in pending_servers {
        let name = server.name.clone();
        let task = mcp.connect_pending_task(server);
        attempts.spawn(name, task);
    }
    if pending_connections.is_empty() {
        mcp.emit_connection_ready().await;
    }

    loop {
        select! {
            () = snapshots.closed() => break,
            Some(command) = command_rx.recv() => match command {
                #[cfg(feature = "oauth")]
                ManagerCommand::AuthenticateServer { name, tx } => authenticate(name, tx, &mut mcp, &mut attempts).await,
            },
            Some(joined) = attempts.join_next(), if !attempts.is_empty() => {
                match joined {
                    Ok(attempt) => {
                        let name = attempt.name.clone();
                        let was_bootstrap = pending_connections.remove(&name);
                        mcp.apply_connection_attempt(attempt).await;
                        if let Some(retry) = mcp.reconnect_task(&name) {
                            attempts.spawn(name, retry);
                        }
                        if was_bootstrap && pending_connections.is_empty() {
                            mcp.emit_connection_ready().await;
                        }
                    }
                    Err(error) => tracing::error!("MCP connection attempt did not complete normally: {error:?}"),
                }
            }
            Some(request) = tool_refresh_rx.recv() => {
                tool_refreshes.spawn(request.refresh());
            }
            Some(joined) = tool_refreshes.join_next(), if !tool_refreshes.is_empty() => {
                match joined {
                    Ok(refresh) => mcp.apply_tool_list_refresh(refresh).await,
                    Err(error) => tracing::warn!(%error, "MCP tool refresh task did not complete normally"),
                }
            }
        }
    }

    attempts.shutdown().await;
    tool_refreshes.abort_all();
    while tool_refreshes.join_next().await.is_some() {}
    mcp.shutdown().await;
    tracing::debug!("MCP manager task ended");
}

#[cfg(feature = "oauth")]
async fn authenticate(
    name: String,
    tx: oneshot::Sender<Result<(), McpError>>,
    mcp: &mut McpManager,
    attempts: &mut McpConnectionAttemptManager,
) {
    match mcp.authenticate_server_task(&name).await {
        Ok(task) => {
            let server_name = name.clone();
            attempts.spawn(name, async move {
                match tokio::time::timeout(MCP_AUTH_TIMEOUT, task).await {
                    Ok(attempt) => attempt,
                    Err(_) => McpConnectAttempt::failed(
                        server_name,
                        McpError::ConnectionFailed("authentication timed out after 3 minutes".to_string()),
                    ),
                }
            });
            let _ = tx.send(Ok(()));
        }
        Err(error) => {
            let _ = tx.send(Err(error));
        }
    }
}
