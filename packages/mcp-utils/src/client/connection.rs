use super::{
    McpClientEvent, McpError, OAuthHandlerFactory, Result,
    config::{McpServer, McpTransport},
    mcp_client::McpClient,
    oauth::{create_auth_manager_from_store, perform_oauth_flow},
};
use crate::transport::create_in_memory_transport;
use rmcp::{
    RoleClient, RoleServer, ServiceExt,
    model::{ClientInfo, Root, Tool as RmcpTool},
    serve_client,
    service::{DynService, RunningService},
    transport::{
        StreamableHttpClientTransport, TokioChildProcess, auth::AuthClient,
        streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::{
    process::Command,
    sync::{RwLock, mpsc},
    task::JoinHandle,
};

#[derive(Debug, Clone)]
pub struct ServerInstructions {
    pub server_name: String,
    pub instructions: String,
}

#[derive(Debug, Clone)]
pub struct Tool {
    pub description: String,
    pub parameters: Value,
}

impl From<RmcpTool> for Tool {
    fn from(tool: RmcpTool) -> Self {
        Self {
            description: tool.description.unwrap_or_default().to_string(),
            parameters: serde_json::Value::Object((*tool.input_schema).clone()),
        }
    }
}

impl From<&RmcpTool> for Tool {
    fn from(tool: &RmcpTool) -> Self {
        Self {
            description: tool.description.clone().unwrap_or_default().to_string(),
            parameters: serde_json::Value::Object((*tool.input_schema).clone()),
        }
    }
}

pub(super) struct ConnectContext<'a> {
    pub client_info: &'a ClientInfo,
    pub event_sender: &'a mpsc::Sender<McpClientEvent>,
    pub roots: &'a Arc<RwLock<Vec<Root>>>,
    pub oauth_handler_factory: Option<&'a OAuthHandlerFactory>,
}

pub struct ConnectedServer {
    pub name: String,
    pub conn: McpServerConnection,
    pub reauth_config: Option<StreamableHttpClientTransportConfig>,
    pub proxy: bool,
}

pub enum ConnectionError {
    NeedsOAuth { name: String, config: StreamableHttpClientTransportConfig, error: McpError, proxy: bool },
    Failed { name: String, error: McpError, proxy: bool },
}

pub struct McpServerConnection {
    pub(super) client: Arc<RunningService<RoleClient, McpClient>>,
    pub(super) server_task: Option<JoinHandle<()>>,
    pub(super) instructions: Option<String>,
}

impl McpServerConnection {
    pub(super) async fn reconnect_with_auth(
        name: &str,
        config: StreamableHttpClientTransportConfig,
        auth_client: AuthClient<reqwest::Client>,
        mcp_client: McpClient,
    ) -> Result<Self> {
        let transport = StreamableHttpClientTransport::with_client(auth_client, config);
        let client = serve_client(mcp_client, transport)
            .await
            .map_err(|e| McpError::ConnectionFailed(format!("reconnect failed for '{name}': {e}")))?;
        Ok(Self::from_parts(client, None))
    }

    pub(super) async fn list_tools(&self) -> Result<Vec<RmcpTool>> {
        let response = self
            .client
            .list_tools(None)
            .await
            .map_err(|e| McpError::ToolDiscoveryFailed(format!("Failed to list tools: {e}")))?;
        Ok(response.tools)
    }

    fn from_parts(client: RunningService<RoleClient, McpClient>, server_task: Option<JoinHandle<()>>) -> Self {
        let instructions = client.peer_info().and_then(|info| info.instructions.clone()).filter(|s| !s.is_empty());
        Self { client: Arc::new(client), server_task, instructions }
    }
}

pub(super) async fn connect_server(
    server: McpServer,
    ctx: &ConnectContext<'_>,
) -> std::result::Result<ConnectedServer, ConnectionError> {
    let McpServer { name, transport, proxy } = server;
    let reauth_config = reauth_config_for(&transport, ctx.oauth_handler_factory);
    let mcp_client =
        McpClient::new(ctx.client_info.clone(), name.clone(), ctx.event_sender.clone(), Arc::clone(ctx.roots));

    match transport {
        McpTransport::Stdio { command, args, env } => {
            connect_stdio(name, command, args, env, reauth_config, mcp_client, proxy).await
        }
        McpTransport::InMemory { server } => connect_in_memory(name, server, reauth_config, mcp_client, proxy).await,
        McpTransport::Http { config } => {
            connect_http(name, config, reauth_config, mcp_client, ctx.oauth_handler_factory, proxy).await
        }
    }
}

async fn connect_stdio(
    name: String,
    command: String,
    args: Vec<String>,
    env: HashMap<String, String>,
    reauth_config: Option<StreamableHttpClientTransportConfig>,
    mcp_client: McpClient,
    proxy: bool,
) -> std::result::Result<ConnectedServer, ConnectionError> {
    let cmd = {
        let mut cmd = Command::new(&command);
        cmd.args(&args);
        cmd.envs(&env);
        cmd
    };

    let child = match TokioChildProcess::new(cmd) {
        Ok(child) => child,
        Err(e) => {
            return Err(ConnectionError::Failed {
                name,
                error: McpError::SpawnFailed { command, reason: e.to_string() },
                proxy,
            });
        }
    };

    match mcp_client.serve(child).await {
        Ok(client) => Ok(connected_server(name, McpServerConnection::from_parts(client, None), reauth_config, proxy)),
        Err(e) => Err(ConnectionError::Failed { name, error: McpError::from(e), proxy }),
    }
}

async fn connect_in_memory(
    name: String,
    server: Box<dyn DynService<RoleServer>>,
    reauth_config: Option<StreamableHttpClientTransportConfig>,
    mcp_client: McpClient,
    proxy: bool,
) -> std::result::Result<ConnectedServer, ConnectionError> {
    match serve_in_memory(server, mcp_client, &name).await {
        Ok((client, handle)) => {
            Ok(connected_server(name, McpServerConnection::from_parts(client, Some(handle)), reauth_config, proxy))
        }
        Err(error) => Err(ConnectionError::Failed { name, error, proxy }),
    }
}

async fn connect_http(
    name: String,
    config: StreamableHttpClientTransportConfig,
    reauth_config: Option<StreamableHttpClientTransportConfig>,
    mcp_client: McpClient,
    oauth_handler_factory: Option<&OAuthHandlerFactory>,
    proxy: bool,
) -> std::result::Result<ConnectedServer, ConnectionError> {
    let conn_err = |e| McpError::ConnectionFailed(format!("HTTP MCP server {name}: {e}"));

    let result = if config.auth_header.is_none()
        && let Ok(Some(auth_manager)) = create_auth_manager_from_store(&name, &config.uri).await
    {
        tracing::debug!("Using OAuth for server '{name}'");
        let auth_client = AuthClient::new(reqwest::Client::default(), auth_manager);
        let transport = StreamableHttpClientTransport::with_client(auth_client, config.clone());
        serve_client(mcp_client, transport).await.map_err(conn_err)
    } else {
        let transport = StreamableHttpClientTransport::from_config(config.clone());
        serve_client(mcp_client, transport).await.map_err(conn_err)
    };

    match result {
        Ok(client) => Ok(connected_server(name, McpServerConnection::from_parts(client, None), reauth_config, proxy)),
        Err(err) => {
            tracing::warn!("Failed to connect to MCP server '{name}': {err}");
            if oauth_handler_factory.is_some() && config.auth_header.is_none() {
                Err(ConnectionError::NeedsOAuth { name, config, error: err, proxy })
            } else {
                Err(ConnectionError::Failed { name, error: err, proxy })
            }
        }
    }
}

pub async fn authenticate_http(
    name: String,
    config: StreamableHttpClientTransportConfig,
    client_info: ClientInfo,
    event_sender: mpsc::Sender<McpClientEvent>,
    roots: Arc<RwLock<Vec<Root>>>,
    oauth_handler_factory: OAuthHandlerFactory,
    proxy: bool,
) -> std::result::Result<ConnectedServer, ConnectionError> {
    let conn = match async {
        let handler = oauth_handler_factory()?;
        let auth_client = perform_oauth_flow(&name, &config.uri, handler.as_ref())
            .await
            .map_err(|e| McpError::ConnectionFailed(format!("OAuth failed for '{name}': {e}")))?;

        let mcp_client = McpClient::new(client_info, name.clone(), event_sender, roots);
        McpServerConnection::reconnect_with_auth(&name, config.clone(), auth_client, mcp_client).await
    }
    .await
    {
        Ok(conn) => conn,
        Err(error) => return Err(ConnectionError::Failed { name, error, proxy }),
    };

    Ok(connected_server(name, conn, Some(config), proxy))
}

fn connected_server(
    name: String,
    conn: McpServerConnection,
    reauth_config: Option<StreamableHttpClientTransportConfig>,
    proxy: bool,
) -> ConnectedServer {
    ConnectedServer { name, conn, reauth_config, proxy }
}

fn reauth_config_for(
    transport: &McpTransport,
    oauth_handler_factory: Option<&OAuthHandlerFactory>,
) -> Option<StreamableHttpClientTransportConfig> {
    match transport {
        McpTransport::Http { config } if oauth_handler_factory.is_some() && config.auth_header.is_none() => {
            Some(config.clone())
        }
        _ => None,
    }
}

async fn serve_in_memory(
    server: Box<dyn DynService<RoleServer>>,
    mcp_client: McpClient,
    label: &str,
) -> Result<(RunningService<RoleClient, McpClient>, JoinHandle<()>)> {
    let (client_transport, server_transport) = create_in_memory_transport();

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

    let client = serve_client(mcp_client, client_transport)
        .await
        .map_err(|e| McpError::ConnectionFailed(format!("Failed to connect to in-memory server '{label}': {e}")))?;

    Ok((client, server_handle))
}
