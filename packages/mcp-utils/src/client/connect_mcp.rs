use super::{
    McpError, Result,
    config::{McpServerConfig, ServerConfig},
    connection::{ConnectParams, ConnectResult, McpServerConnection},
    manager::McpClientEvent,
    mcp_client::McpClient,
    oauth::OAuthHandler,
    tool_proxy::ToolProxy,
};
use rmcp::{
    model::{ClientInfo, Root, Tool as RmcpTool},
    transport::streamable_http_client::StreamableHttpClientTransportConfig,
};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};

/// Whether a server's tools should be directly exposed to the agent or only
/// registered internally for proxy routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Registration {
    /// Tools are added to `tool_definitions` (visible to the agent).
    Direct,
    /// Tools are stored in `tools` for routing but not exposed to the agent.
    Proxied,
}

/// A single leaf server to connect — either top-level or nested inside a proxy.
pub(super) struct ConnectionSpec {
    pub(super) name: String,
    pub(super) config: ServerConfig,
    pub(super) proxy: Option<String>,
    pub(super) registration: Registration,
}

/// Pre-work done once per tool-proxy before any connects run.
pub(super) struct ProxySpec {
    pub(super) name: String,
    pub(super) tool_dir: PathBuf,
}

/// Result of one leaf's parallel connect + `list_tools`.
pub(super) enum ConnectOutcome {
    Ready {
        name: String,
        conn: McpServerConnection,
        tools: Vec<RmcpTool>,
        proxy: Option<String>,
        registration: Registration,
        reauth_config: Option<StreamableHttpClientTransportConfig>,
    },
    NeedsOAuth {
        name: String,
        config: StreamableHttpClientTransportConfig,
        error: McpError,
        proxy: Option<String>,
    },
    Failed {
        name: String,
        error: McpError,
    },
}

/// Flatten `Vec<McpServerConfig>` into leaf connects + per-proxy pre-work.
///
/// For each tool-proxy we resolve its `tool_dir` and run `clean_dir` up front,
/// before any connects — `clean_dir` wipes the directory and the proxy finalize
/// stage writes into it.
pub(super) async fn build_plan(configs: Vec<McpServerConfig>) -> Result<(Vec<ConnectionSpec>, Vec<ProxySpec>)> {
    let mut direct = Vec::new();
    let mut proxies = Vec::new();

    for config in configs {
        match config {
            McpServerConfig::Server(server) => {
                direct.push(ConnectionSpec {
                    name: server.name().to_string(),
                    config: server,
                    proxy: None,
                    registration: Registration::Direct,
                });
            }
            McpServerConfig::ToolProxy { name, servers } => {
                let tool_dir = ToolProxy::dir(&name)?;
                ToolProxy::clean_dir(&tool_dir).await?;

                for server in servers {
                    direct.push(ConnectionSpec {
                        name: server.name().to_string(),
                        config: server,
                        proxy: Some(name.clone()),
                        registration: Registration::Proxied,
                    });
                }
                proxies.push(ProxySpec { name, tool_dir });
            }
        }
    }

    Ok((direct, proxies))
}

/// Derive re-auth config from the server config and OAuth handler availability.
pub(super) fn get_reauth_config(
    config: &ServerConfig,
    oauth_handler: Option<&Arc<dyn OAuthHandler>>,
) -> Option<StreamableHttpClientTransportConfig> {
    match config {
        ServerConfig::Http { config, .. } if oauth_handler.is_some() && config.auth_header.is_none() => {
            Some(config.clone())
        }
        _ => None,
    }
}

/// Connect a single leaf and discover its tools. Parallel-safe: takes no
/// `&self` borrow on the manager, only immutable references to shared state.
pub(super) async fn connect_mcp(
    leaf: ConnectionSpec,
    client_info: &ClientInfo,
    event_sender: &mpsc::Sender<McpClientEvent>,
    roots: &Arc<RwLock<Vec<Root>>>,
    oauth_handler: Option<&Arc<dyn OAuthHandler>>,
) -> ConnectOutcome {
    let ConnectionSpec { name, config, proxy, registration } = leaf;
    let reauth_config = get_reauth_config(&config, oauth_handler);
    let mcp_client = McpClient::new(client_info.clone(), name.clone(), event_sender.clone(), Arc::clone(roots));
    let params = ConnectParams { mcp_client, oauth_handler: oauth_handler.cloned() };
    match McpServerConnection::connect(config, params).await {
        ConnectResult::Connected(conn) => match conn.list_tools().await {
            Ok(tools) => ConnectOutcome::Ready { name, conn, tools, proxy, registration, reauth_config },
            Err(e) => ConnectOutcome::Failed {
                error: McpError::ToolDiscoveryFailed(format!("Failed to list tools for {name}: {e}")),
                name,
            },
        },
        ConnectResult::NeedsOAuth { name, config, error } => ConnectOutcome::NeedsOAuth { name, config, error, proxy },
        ConnectResult::Failed(e) => ConnectOutcome::Failed { name, error: e },
    }
}
