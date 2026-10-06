use super::{
    McpClientEvent, McpError, Result,
    config::McpHttpConfig,
    manager::{RuntimeMcpServer, RuntimeMcpTransport, ToolListChangedRequest},
    mcp_client::McpClient,
};
use crate::protocol::client_lifecycle_mode;
use rmcp::{
    RoleClient, RoleServer, ServiceExt,
    model::{ClientConfig, Tool as RmcpTool},
    serve_client_with_lifecycle,
    service::{ClientInitializeError, DynService, RunningService},
};
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use tokio::{sync::mpsc, task::JoinHandle};

#[cfg(feature = "oauth")]
use super::oauth::{self, OAuthHandlerFactory, connect_http};
#[cfg(feature = "stdio")]
use super::stdio::connect_stdio;
#[cfg(feature = "oauth")]
use aether_auth::OAuthCredentialStorage;
#[cfg(not(feature = "oauth"))]
use rmcp::transport::StreamableHttpClientTransport;

pub(super) struct ConnectConfig {
    pub client_info: ClientConfig,
    pub event_sender: Option<mpsc::Sender<McpClientEvent>>,
    pub tool_refresh_sender: mpsc::Sender<ToolListChangedRequest>,
    pub next_connection_generation: Arc<AtomicU64>,
    #[cfg_attr(not(feature = "stdio"), allow(dead_code))]
    pub root_dir: PathBuf,
    #[cfg(feature = "oauth")]
    pub oauth_handler_factory: Option<OAuthHandlerFactory>,
    #[cfg(feature = "oauth")]
    pub oauth_credential_store: Option<Arc<dyn OAuthCredentialStorage>>,
}

/// The result of attempting to connect (or authenticate) to an MCP server.
pub struct McpConnectAttempt {
    pub name: String,
    pub outcome: McpConnectOutcome,
}

pub enum McpConnectOutcome {
    Connected { conn: McpServerConnection, reauth_config: Option<McpHttpConfig> },
    NeedsOAuth { config: McpHttpConfig, challenge: Option<String>, error: McpError },
    Failed { error: McpError },
}

impl McpConnectAttempt {
    pub fn failed(name: impl Into<String>, error: McpError) -> Self {
        Self { name: name.into(), outcome: McpConnectOutcome::Failed { error } }
    }
}

pub struct McpServerConnection {
    pub(super) client: Arc<RunningService<RoleClient, McpClient>>,
    pub(super) server_task: Option<JoinHandle<()>>,
    pub(super) instructions: Option<String>,
    generation: u64,
}

impl McpServerConnection {
    pub(super) async fn list_tools(&self) -> Result<Vec<RmcpTool>> {
        self.client
            .list_all_tools()
            .await
            .map_err(|e| McpError::ToolDiscoveryFailed(format!("Failed to list tools: {e}")))
    }

    pub(super) fn from_parts(
        client: RunningService<RoleClient, McpClient>,
        server_task: Option<JoinHandle<()>>,
        generation: u64,
    ) -> Self {
        let instructions = client.peer_info().and_then(|info| info.instructions.clone()).filter(|s| !s.is_empty());
        Self { client: Arc::new(client), server_task, instructions, generation }
    }

    pub(super) fn generation(&self) -> u64 {
        self.generation
    }
}

pub(super) async fn connect_server(server: RuntimeMcpServer, ctx: &ConnectConfig) -> McpConnectAttempt {
    let RuntimeMcpServer { name, transport, tool_exposure: _ } = server;
    #[cfg(feature = "oauth")]
    let reauth_config = oauth::reauth_config_for(&transport, ctx);
    #[cfg(not(feature = "oauth"))]
    let reauth_config = None;
    let generation = ctx.next_connection_generation.fetch_add(1, Ordering::Relaxed);
    let mcp_client = new_client(ctx, &name, generation);

    let outcome = match transport {
        #[cfg(feature = "stdio")]
        RuntimeMcpTransport::Stdio { command, args, env } => {
            connect_stdio(&name, command, args, env, mcp_client, ctx.root_dir.clone(), generation).await
        }
        #[cfg(not(feature = "stdio"))]
        RuntimeMcpTransport::Stdio { command, .. } => {
            let reason = "stdio servers require the aether-mcp-utils `stdio` feature".to_string();
            McpConnectOutcome::Failed { error: McpError::SpawnFailed { command, reason } }
        }
        RuntimeMcpTransport::InMemory { server } => connect_in_memory(&name, server, mcp_client, generation).await,
        RuntimeMcpTransport::Http(config) => connect_http(&name, config, mcp_client, ctx, generation).await,
    };

    McpConnectAttempt { name, outcome: outcome.with_reauth(reauth_config) }
}

pub(super) fn new_client(ctx: &ConnectConfig, name: &str, generation: u64) -> McpClient {
    let client = McpClient::new(ctx.client_info.clone(), name.to_string())
        .with_tool_refresh(ctx.tool_refresh_sender.clone(), generation);
    match &ctx.event_sender {
        Some(sender) => client.with_event_sender(sender.clone()),
        None => client,
    }
}

impl McpConnectOutcome {
    fn with_reauth(self, reauth_config: Option<McpHttpConfig>) -> Self {
        match self {
            Self::Connected { conn, .. } => Self::Connected { conn, reauth_config },
            other => other,
        }
    }
}

#[cfg(not(feature = "oauth"))]
async fn connect_http(
    name: &str,
    config: McpHttpConfig,
    mcp_client: McpClient,
    _ctx: &ConnectConfig,
    generation: u64,
) -> McpConnectOutcome {
    let transport = StreamableHttpClientTransport::from_config(config.transport);
    match serve_client_with_lifecycle(mcp_client, transport, client_lifecycle_mode()).await {
        Ok(client) => McpConnectOutcome::Connected {
            conn: McpServerConnection::from_parts(client, None, generation),
            reauth_config: None,
        },
        Err(error) => McpConnectOutcome::Failed { error: http_connection_error(name, &error) },
    }
}

pub(super) fn http_connection_error(name: &str, error: &ClientInitializeError) -> McpError {
    let error = McpError::ConnectionFailed(format!("HTTP MCP server {name}: {error}"));
    tracing::warn!("Failed to connect to MCP server '{name}': {error}");
    error
}

async fn connect_in_memory(
    name: &str,
    server: Box<dyn DynService<RoleServer>>,
    mcp_client: McpClient,
    generation: u64,
) -> McpConnectOutcome {
    match serve_in_memory(server, mcp_client, name).await {
        Ok((client, handle)) => McpConnectOutcome::Connected {
            conn: McpServerConnection::from_parts(client, Some(handle), generation),
            reauth_config: None,
        },
        Err(error) => McpConnectOutcome::Failed { error },
    }
}

async fn serve_in_memory(
    server: Box<dyn DynService<RoleServer>>,
    mcp_client: McpClient,
    label: &str,
) -> Result<(RunningService<RoleClient, McpClient>, JoinHandle<()>)> {
    let (client_transport, server_transport) = tokio::io::duplex(64 * 1024);

    let server_handle = tokio::spawn(async move {
        match server.serve(server_transport).await {
            Ok(_service) => {
                std::future::pending::<()>().await;
            }
            Err(e) => {
                eprintln!("MCP server error: {e}");
            }
        }
    });

    let client = serve_client_with_lifecycle(mcp_client, client_transport, client_lifecycle_mode())
        .await
        .map_err(|e| McpError::ConnectionFailed(format!("Failed to connect to in-memory server '{label}': {e}")))?;

    Ok((client, server_handle))
}
