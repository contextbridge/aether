use super::{McpConnectAttempt, McpConnectionAttemptManager, McpError, McpManager, RuntimeMcpServer};
use std::{collections::HashSet, time::Duration};
use tokio::{
    select,
    sync::{mpsc, oneshot},
    task::{JoinHandle, JoinSet},
};

#[derive(Debug)]
pub enum ManagerCommand {
    AuthenticateServer { name: String, tx: oneshot::Sender<Result<(), McpError>> },
    Shutdown,
}

/// Owns connection bootstrap, refresh work, and orderly backend shutdown.
pub struct ConnectionRuntime {
    commands: mpsc::Sender<ManagerCommand>,
    task: JoinHandle<()>,
}

impl ConnectionRuntime {
    pub fn spawn(manager: McpManager, pending: Vec<RuntimeMcpServer>) -> Self {
        let (commands, receiver) = mpsc::channel(32);
        Self { commands, task: tokio::spawn(run_mcp_task(manager, receiver, pending)) }
    }

    pub fn commands(&self) -> mpsc::Sender<ManagerCommand> {
        self.commands.clone()
    }

    pub async fn shutdown(&mut self) -> Result<(), tokio::task::JoinError> {
        let _ = self.commands.send(ManagerCommand::Shutdown).await;
        (&mut self.task).await
    }
}

impl Drop for ConnectionRuntime {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub async fn run_mcp_task(
    mut mcp: McpManager,
    mut command_rx: mpsc::Receiver<ManagerCommand>,
    pending_servers: Vec<RuntimeMcpServer>,
) {
    let mut tool_refresh_rx = mcp.take_tool_refresh_receiver();
    let mut tool_refreshes = JoinSet::new();
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
                match command {
                    Some(ManagerCommand::AuthenticateServer { name, tx }) => {
                        authenticate(name, tx, &mut mcp, &mut mcp_connection_attempts).await;
                    }
                    Some(ManagerCommand::Shutdown) | None => break,
                }
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
                    Err(error) => tracing::error!("MCP connection task did not complete normally: {error:?}"),
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
    mcp_connection_attempts.shutdown().await;
    tool_refreshes.abort_all();
    while tool_refreshes.join_next().await.is_some() {}
    mcp.shutdown().await;
    tracing::debug!("MCP manager task ended");
}

const MCP_AUTH_TIMEOUT: Duration = Duration::from_mins(3);

async fn authenticate(
    name: String,
    tx: oneshot::Sender<Result<(), McpError>>,
    mcp: &mut McpManager,
    auth_tasks: &mut McpConnectionAttemptManager,
) {
    match mcp.authenticate_server_task(&name).await {
        Ok(task) => {
            let server_name = name.clone();
            auth_tasks.spawn(name, async move {
                match tokio::time::timeout(MCP_AUTH_TIMEOUT, task).await {
                    Ok(attempt) => attempt,
                    Err(_) => McpConnectAttempt::failed(
                        server_name,
                        McpError::ConnectionFailed("authentication timed out after 3 minutes".into()),
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
