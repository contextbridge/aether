use mcp_utils::client::{McpConnectAttempt, McpConnectionAttemptManager, McpError, McpManager, RuntimeMcpServer};
use std::collections::HashSet;
use std::time::Duration;
use tokio::select;
use tokio::sync::{mpsc, oneshot};

const MCP_AUTH_TIMEOUT: Duration = Duration::from_mins(3);

#[derive(Debug)]
pub(super) enum ManagerCommand {
    AuthenticateServer { name: String, tx: oneshot::Sender<Result<(), McpError>> },
}

pub(super) async fn run_mcp_task(
    mut mcp: McpManager,
    mut command_rx: mpsc::Receiver<ManagerCommand>,
    pending_servers: Vec<RuntimeMcpServer>,
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
                    Err(error) => tracing::error!("MCP auth task did not complete normally: {error:?}"),
                }
            }
        }
    }

    mcp_connection_attempts.shutdown().await;
    mcp.shutdown().await;
    tracing::debug!("MCP manager task ended");
}

async fn on_command(command: ManagerCommand, mcp: &mut McpManager, auth_tasks: &mut McpConnectionAttemptManager) {
    match command {
        ManagerCommand::AuthenticateServer { name, tx } => match mcp.authenticate_server_task(&name).await {
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
                let _ = tx.send(Ok(()));
            }
            Err(error) => {
                let _ = tx.send(Err(error));
            }
        },
    }
}
