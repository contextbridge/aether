use super::{
    McpClientEvent, McpError, OAuthHandlerFactory, Result,
    config::McpHttpConfig,
    manager::{RuntimeMcpServer, RuntimeMcpTransport, ToolListChangedRequest},
    mcp_client::McpClient,
};
use crate::{client::OAuthHandlerContext, protocol::client_lifecycle_mode, transport::create_in_memory_transport};
use aether_auth::{OAuthCredentialStorage, create_auth_manager_from_store, perform_oauth_flow};
use llm::ToolAnnotations;
use rmcp::{
    RoleClient, RoleServer, ServiceExt,
    model::{ClientInfo, Tool as RmcpTool},
    serve_client_with_lifecycle,
    service::{DynService, RunningService},
    transport::{
        StreamableHttpClientTransport, TokioChildProcess, auth::AuthClient,
        streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{ChildStderr, Command},
    sync::mpsc,
    task::JoinHandle,
};

#[derive(Debug, Clone)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    pub annotations: Option<ToolAnnotations>,
}

pub(crate) fn convert_tool_annotations(annotations: &rmcp::model::ToolAnnotations) -> ToolAnnotations {
    ToolAnnotations {
        title: annotations.title.clone(),
        read_only_hint: annotations.read_only_hint,
        destructive_hint: annotations.destructive_hint,
        idempotent_hint: annotations.idempotent_hint,
        open_world_hint: annotations.open_world_hint,
    }
}

impl From<RmcpTool> for Tool {
    fn from(tool: RmcpTool) -> Self {
        Self::from(&tool)
    }
}

impl From<&RmcpTool> for Tool {
    fn from(tool: &RmcpTool) -> Self {
        Self {
            name: tool.name.to_string(),
            description: tool.description.clone().unwrap_or_default().to_string(),
            parameters: serde_json::Value::Object((*tool.input_schema).clone()),
            annotations: tool.annotations.as_ref().map(convert_tool_annotations),
        }
    }
}

pub(super) struct ConnectConfig {
    pub client_info: ClientInfo,
    pub event_sender: mpsc::Sender<McpClientEvent>,
    pub tool_refresh_sender: mpsc::Sender<ToolListChangedRequest>,
    pub next_connection_generation: Arc<AtomicU64>,
    pub root_dir: PathBuf,
    pub oauth_handler_factory: Option<OAuthHandlerFactory>,
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
    pub(super) async fn reconnect_with_auth(
        name: &str,
        config: StreamableHttpClientTransportConfig,
        auth_client: AuthClient<reqwest::Client>,
        mcp_client: McpClient,
        generation: u64,
    ) -> Result<Self> {
        let transport = StreamableHttpClientTransport::with_client(auth_client, config);
        let client = serve_client_with_lifecycle(mcp_client, transport, client_lifecycle_mode())
            .await
            .map_err(|e| McpError::ConnectionFailed(format!("reconnect failed for '{name}': {e}")))?;
        Ok(Self::from_parts(client, None, generation))
    }

    pub(super) async fn list_tools(&self) -> Result<Vec<RmcpTool>> {
        let response = self
            .client
            .list_tools(None)
            .await
            .map_err(|e| McpError::ToolDiscoveryFailed(format!("Failed to list tools: {e}")))?;
        Ok(response.tools)
    }

    fn from_parts(
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
    let reauth_config = reauth_config_for(&transport, ctx.oauth_handler_factory.as_ref());
    let generation = ctx.next_connection_generation.fetch_add(1, Ordering::Relaxed);
    let mcp_client = McpClient::new(ctx.client_info.clone(), name.clone(), ctx.event_sender.clone())
        .with_tool_refresh(ctx.tool_refresh_sender.clone(), generation);

    let outcome = match transport {
        RuntimeMcpTransport::Stdio { command, args, env } => {
            connect_stdio(&name, command, args, env, mcp_client, ctx.root_dir.clone(), generation).await
        }
        RuntimeMcpTransport::InMemory { server } => connect_in_memory(&name, server, mcp_client, generation).await,
        RuntimeMcpTransport::Http(config) => {
            connect_http(
                &name,
                config,
                mcp_client,
                ctx.oauth_handler_factory.as_ref(),
                ctx.oauth_credential_store.as_ref(),
                generation,
            )
            .await
        }
    };

    McpConnectAttempt { name, outcome: outcome.with_reauth(reauth_config) }
}

pub async fn authenticate_http(
    name: String,
    config: McpHttpConfig,
    challenge: Option<String>,
    ctx: Arc<ConnectConfig>,
) -> McpConnectAttempt {
    let outcome = match async {
        let factory = ctx
            .oauth_handler_factory
            .as_ref()
            .ok_or_else(|| McpError::ConnectionFailed(format!("No OAuth handler factory available for '{name}'")))?;
        let oauth = config
            .resolved_oauth()
            .ok_or_else(|| McpError::ConnectionFailed(format!("OAuth is not available for '{name}'")))?;
        let handler = factory(OAuthHandlerContext {
            server_name: name.clone(),
            callback_port: Some(oauth.callback_port),
            tx: ctx.event_sender.clone(),
        })?;

        let auth_client = perform_oauth_flow(
            &name,
            &config.transport.uri,
            handler.as_ref(),
            aether_auth::OAuthFlowOptions { client_registration: oauth.client_registration, challenge },
            ctx.oauth_credential_store.clone(),
        )
        .await
        .map_err(|e| McpError::ConnectionFailed(format!("OAuth failed for '{name}': {e}")))?;

        let generation = ctx.next_connection_generation.fetch_add(1, Ordering::Relaxed);
        let mcp_client = McpClient::new(ctx.client_info.clone(), name.clone(), ctx.event_sender.clone())
            .with_tool_refresh(ctx.tool_refresh_sender.clone(), generation);
        McpServerConnection::reconnect_with_auth(&name, config.transport.clone(), auth_client, mcp_client, generation)
            .await
    }
    .await
    {
        Ok(conn) => McpConnectOutcome::Connected { conn, reauth_config: Some(config) },
        Err(error) => McpConnectOutcome::Failed { error },
    };

    McpConnectAttempt { name, outcome }
}

impl McpConnectOutcome {
    fn with_reauth(self, reauth_config: Option<McpHttpConfig>) -> Self {
        match self {
            Self::Connected { conn, .. } => Self::Connected { conn, reauth_config },
            other => other,
        }
    }
}

async fn connect_stdio(
    server_name: &str,
    command: String,
    args: Vec<String>,
    env: HashMap<String, String>,
    mcp_client: McpClient,
    cwd: PathBuf,
    generation: u64,
) -> McpConnectOutcome {
    let mut cmd = Command::new(&command);
    cmd.args(&args).envs(&env).current_dir(&cwd);

    let (proc, stderr) = match TokioChildProcess::builder(cmd).stderr(Stdio::piped()).spawn() {
        Ok(parts) => parts,
        Err(e) => return McpConnectOutcome::Failed { error: McpError::SpawnFailed { command, reason: e.to_string() } },
    };
    let stderr_task = stderr.map(|stderr| spawn_stderr_logger(server_name.to_string(), stderr));

    match serve_client_with_lifecycle(mcp_client, proc, client_lifecycle_mode()).await {
        Ok(client) => McpConnectOutcome::Connected {
            conn: McpServerConnection::from_parts(client, stderr_task, generation),
            reauth_config: None,
        },
        Err(e) => {
            if let Some(task) = stderr_task {
                task.abort();
            }
            McpConnectOutcome::Failed { error: McpError::from(e) }
        }
    }
}

fn spawn_stderr_logger(server_name: String, stderr: ChildStderr) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => tracing::info!(server = %server_name, stderr = %line, "MCP server stderr"),
                Ok(None) => break,
                Err(error) => {
                    tracing::warn!(server = %server_name, %error, "failed to read MCP server stderr");
                    break;
                }
            }
        }
    })
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

async fn connect_http(
    name: &str,
    config: McpHttpConfig,
    mcp_client: McpClient,
    oauth_handler_factory: Option<&OAuthHandlerFactory>,
    oauth_credential_store: Option<&Arc<dyn OAuthCredentialStorage>>,
    generation: u64,
) -> McpConnectOutcome {
    let conn_err = |e| McpError::ConnectionFailed(format!("HTTP MCP server {name}: {e}"));
    let oauth = config.resolved_oauth();
    let restored = if let (Some(store), Some(oauth)) = (oauth_credential_store, oauth.as_ref()) {
        match create_auth_manager_from_store(
            name,
            &config.transport.uri,
            oauth.client_registration.pre_registered_client_id(),
            &oauth.redirect_uri(),
            Arc::clone(store),
        )
        .await
        {
            Ok(manager) => manager,
            Err(e) => {
                tracing::warn!(
                    server = %name,
                    error = %e,
                    "Failed to initialize auth manager from stored credentials, proceeding without auth"
                );
                None
            }
        }
    } else {
        None
    };
    let result = if let Some(manager) = restored {
        tracing::debug!("Using OAuth for server '{name}'");
        let auth_client = AuthClient::new(reqwest::Client::default(), manager);
        let transport = StreamableHttpClientTransport::with_client(auth_client, config.transport.clone());
        serve_client_with_lifecycle(mcp_client, transport, client_lifecycle_mode()).await
    } else {
        let transport = StreamableHttpClientTransport::from_config(config.transport.clone());
        serve_client_with_lifecycle(mcp_client, transport, client_lifecycle_mode()).await
    };

    match result {
        Ok(client) => McpConnectOutcome::Connected {
            conn: McpServerConnection::from_parts(client, None, generation),
            reauth_config: None,
        },
        Err(error) => {
            let challenge = error.auth_challenge().map(str::to_string);
            let authorization_required = error.is_authorization_required();
            let error = conn_err(error);
            tracing::warn!("Failed to connect to MCP server '{name}': {error}");
            if oauth_handler_factory.is_some() && oauth.is_some() && (authorization_required || challenge.is_some()) {
                McpConnectOutcome::NeedsOAuth { config, challenge, error }
            } else {
                McpConnectOutcome::Failed { error }
            }
        }
    }
}

fn reauth_config_for(
    transport: &RuntimeMcpTransport,
    oauth_handler_factory: Option<&OAuthHandlerFactory>,
) -> Option<McpHttpConfig> {
    match transport {
        RuntimeMcpTransport::Http(config)
            if oauth_handler_factory.is_some() && config.transport.auth_header.is_none() =>
        {
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

    let client = serve_client_with_lifecycle(mcp_client, client_transport, client_lifecycle_mode())
        .await
        .map_err(|e| McpError::ConnectionFailed(format!("Failed to connect to in-memory server '{label}': {e}")))?;

    Ok((client, server_handle))
}
