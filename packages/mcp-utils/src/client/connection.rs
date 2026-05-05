use super::{McpError, Result, config::ServerConfig, mcp_client::McpClient};
use crate::transport::create_in_memory_transport;
use rmcp::{
    RoleClient, RoleServer, ServiceExt,
    model::Tool as RmcpTool,
    serve_client,
    service::{DynService, RunningService},
    transport::{
        StreamableHttpClientTransport, TokioChildProcess, auth::AuthClient,
        streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use serde_json::Value;
use std::sync::Arc;
use tokio::{process::Command, task::JoinHandle};

use super::oauth::{OAuthHandler, create_auth_manager_from_store};

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

/// Everything the connection needs from the manager.
pub(super) struct ConnectParams {
    pub mcp_client: McpClient,
    pub oauth_handler: Option<Arc<dyn OAuthHandler>>,
}

/// Result of attempting to connect to an MCP server.
pub(super) enum ConnectResult {
    /// Connection established successfully.
    Connected(McpServerConnection),
    /// HTTP server failed; may need OAuth. Carries the config for retry.
    NeedsOAuth { name: String, config: StreamableHttpClientTransportConfig, error: McpError },
    /// Hard failure (non-HTTP, or no OAuth handler available).
    Failed(McpError),
}

pub(super) struct McpServerConnection {
    pub(super) client: Arc<RunningService<RoleClient, McpClient>>,
    pub(super) server_task: Option<JoinHandle<()>>,
    pub(super) instructions: Option<String>,
}

impl McpServerConnection {
    /// Connect to an MCP server described by `config`.
    ///
    /// This is the single entry point for establishing a connection — handling
    /// transport creation, OAuth credential lookup, `serve_client()`, and
    /// returning a ready-to-use connection.
    pub(super) async fn connect(config: ServerConfig, params: ConnectParams) -> ConnectResult {
        match config {
            ServerConfig::Stdio { command, args, .. } => {
                let mut cmd = Command::new(&command);
                cmd.args(&args);
                let child = match TokioChildProcess::new(cmd) {
                    Ok(child) => child,
                    Err(e) => {
                        return ConnectResult::Failed(McpError::SpawnFailed { command, reason: e.to_string() });
                    }
                };
                match params.mcp_client.serve(child).await {
                    Ok(client) => ConnectResult::Connected(Self::from_parts(client, None)),
                    Err(e) => ConnectResult::Failed(McpError::from(e)),
                }
            }

            ServerConfig::InMemory { name, server } => match serve_in_memory(server, params.mcp_client, &name).await {
                Ok((client, handle)) => ConnectResult::Connected(Self::from_parts(client, Some(handle))),
                Err(e) => ConnectResult::Failed(e),
            },

            ServerConfig::Http { name, config: cfg } => Self::connect_http(name, cfg, params).await,
        }
    }

    /// Reconnect to an HTTP server using an already-obtained `AuthClient`.
    ///
    /// Used after a successful OAuth flow to establish the authenticated connection.
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

    /// List tools from the connected server.
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

    /// Connect to an HTTP MCP server. Tries stored OAuth credentials first,
    /// falls back to plain connection, and returns `NeedsOAuth` on failure
    /// if an OAuth handler is available.
    async fn connect_http(
        name: String,
        config: StreamableHttpClientTransportConfig,
        params: ConnectParams,
    ) -> ConnectResult {
        let conn_err = |e| McpError::ConnectionFailed(format!("HTTP MCP server {name}: {e}"));

        let result = if config.auth_header.is_none() {
            if let Some(auth_client) = create_auth_client(&name, &config.uri).await {
                tracing::debug!("Using OAuth for server '{name}'");
                let transport = StreamableHttpClientTransport::with_client(auth_client, config.clone());
                serve_client(params.mcp_client, transport).await.map_err(conn_err)
            } else {
                let transport = StreamableHttpClientTransport::from_config(config.clone());
                serve_client(params.mcp_client, transport).await.map_err(conn_err)
            }
        } else {
            let transport = StreamableHttpClientTransport::from_config(config.clone());
            serve_client(params.mcp_client, transport).await.map_err(conn_err)
        };

        match result {
            Ok(client) => ConnectResult::Connected(Self::from_parts(client, None)),
            Err(err) => {
                tracing::warn!("Failed to connect to MCP server '{name}': {err}");
                if params.oauth_handler.is_some() && config.auth_header.is_none() {
                    ConnectResult::NeedsOAuth { name, config, error: err }
                } else {
                    ConnectResult::Failed(err)
                }
            }
        }
    }
}

/// Try to build an `AuthClient` from stored OAuth credentials.
async fn create_auth_client(server_id: &str, base_url: &str) -> Option<AuthClient<reqwest::Client>> {
    let auth_manager = create_auth_manager_from_store(server_id, base_url).await.ok()??;
    Some(AuthClient::new(reqwest::Client::default(), auth_manager))
}

/// Spawn an in-memory MCP server on a background task and connect a client to it.
///
/// Returns the running client service and the server's join handle.
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
