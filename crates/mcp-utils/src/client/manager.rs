use super::{
    McpError, McpSnapshot, Result,
    config::{McpHttpConfig, McpServer, McpTransport, ToolExposure},
    connection::{ConnectConfig, McpConnectAttempt, McpConnectOutcome, McpServerConnection, connect_server},
    manager_task::{self, ManagerCommand},
    mcp_client::client_capabilities,
    mcp_handle::McpHandle,
    naming::{SERVERNAME_DELIMITER, create_namespaced_tool_name, split_on_server_name},
    tool_catalog::{ServerCatalogEntry, ToolCatalog},
    tool_filter::ToolFilter,
};
use futures::future::join_all;
use rmcp::{
    Peer, RoleClient, RoleServer,
    model::{ClientCapabilities, ClientConfig, ElicitRequestParams, ElicitResult, Implementation, Tool as RmcpTool},
    service::DynService,
};
use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, atomic::AtomicU64};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;

use utils::mcp_status::{McpServerAuthCapability, McpServerStatus, McpServerStatusEntry};

#[cfg(feature = "oauth")]
use super::oauth::{OAuthHandlerFactory, authenticate_http};
#[cfg(feature = "oauth")]
use aether_auth::OAuthCredentialStorage;

pub struct RuntimeMcpServer {
    pub name: String,
    pub transport: RuntimeMcpTransport,
    pub tool_exposure: ToolExposure,
}

pub enum RuntimeMcpTransport {
    Stdio { command: String, args: Vec<String>, env: HashMap<String, String> },
    Http(McpHttpConfig),
    InMemory { server: Box<dyn DynService<RoleServer>> },
}

impl RuntimeMcpServer {
    pub fn new(name: impl Into<String>, transport: RuntimeMcpTransport, tool_exposure: ToolExposure) -> Self {
        Self { name: name.into(), transport, tool_exposure }
    }

    pub fn with_exposure(mut self, exposure: ToolExposure) -> Self {
        self.tool_exposure = exposure;
        self
    }
}

/// Converts parsed configuration into a connectable server. In-memory servers need a
/// host-provided factory, so they are rejected here.
impl TryFrom<McpServer> for RuntimeMcpServer {
    type Error = McpError;

    fn try_from(server: McpServer) -> Result<Self> {
        let McpServer { name, transport, tool_exposure } = server;
        let transport = match transport {
            McpTransport::Stdio { command, args, env } => RuntimeMcpTransport::Stdio { command, args, env },
            McpTransport::Http(config) => RuntimeMcpTransport::Http(config),
            McpTransport::InMemory { spec } => {
                return Err(McpError::InMemoryFactoryNotFound { server: name, factory: spec.factory });
            }
        };
        Ok(Self::new(name, transport, tool_exposure))
    }
}

#[derive(Debug)]
pub struct ElicitationRequest {
    pub server_name: String,
    pub request: ElicitRequestParams,
    pub response_sender: oneshot::Sender<ElicitResult>,
}

/// Events emitted by MCP clients that require attention from the host
/// (e.g. the relay or TUI). Flows through a single channel from `McpManager`
/// to the consumer.
#[derive(Debug)]
pub enum McpClientEvent {
    Elicitation(Box<ElicitationRequest>),
    ElicitationComplete { server_name: String, elicitation_id: String },
    ServerStatusesChanged(Vec<McpServerStatusEntry>),
    AuthenticationFailed { server: String, error: String },
    ConnectionReady(McpConnectionDetails),
}

pub type McpConnectionDetails = Arc<McpSnapshot>;

/// Exponential backoff for retrying HTTP servers whose connection attempt failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReconnectPolicy {
    pub initial_delay: Duration,
    pub max_delay: Duration,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self { initial_delay: Duration::from_secs(1), max_delay: Duration::from_mins(1) }
    }
}

impl ReconnectPolicy {
    fn delay(self, failures: u32) -> Duration {
        self.initial_delay.saturating_mul(2u32.saturating_pow(failures)).min(self.max_delay)
    }
}

/// Manages connections to multiple MCP servers and their tools.
///
/// Use it directly with [`Self::add_mcps`], or take a [`Self::handle`] and
/// [`Self::spawn`] it to connect servers in the background and keep their
/// catalogs current.
pub struct McpManager {
    servers: HashMap<String, ServerRecord>,
    catalog: ToolCatalog,
    tool_filter: ToolFilter,
    client_info: ClientConfig,
    event_sender: Option<mpsc::Sender<McpClientEvent>>,
    root_dir: PathBuf,
    #[cfg(feature = "oauth")]
    oauth_handler_factory: Option<OAuthHandlerFactory>,
    #[cfg(feature = "oauth")]
    oauth_credential_store: Option<Arc<dyn OAuthCredentialStorage>>,
    snapshot_sender: watch::Sender<Arc<McpSnapshot>>,
    command_sender: mpsc::Sender<ManagerCommand>,
    command_receiver: Option<mpsc::Receiver<ManagerCommand>>,
    tool_refresh_sender: mpsc::Sender<ToolListChangedRequest>,
    tool_refresh_receiver: Option<mpsc::Receiver<ToolListChangedRequest>>,
    next_connection_generation: Arc<AtomicU64>,
    progressive_discovery_instructions: Option<String>,
    reconnect_policy: Option<ReconnectPolicy>,
}

impl Default for McpManager {
    fn default() -> Self {
        Self::new()
    }
}

impl McpManager {
    pub fn new() -> Self {
        let (tool_refresh_sender, tool_refresh_receiver) = mpsc::channel(32);
        let (command_sender, command_receiver) = mpsc::channel(32);
        let (snapshot_sender, _) = watch::channel(Arc::new(McpSnapshot::default()));
        Self {
            servers: HashMap::new(),
            catalog: ToolCatalog::new(),
            tool_filter: ToolFilter::default(),
            client_info: ClientConfig::new(client_capabilities(), Implementation::new("aether", "0.1.0")),
            event_sender: None,
            root_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            #[cfg(feature = "oauth")]
            oauth_handler_factory: None,
            #[cfg(feature = "oauth")]
            oauth_credential_store: None,
            snapshot_sender,
            command_sender,
            command_receiver: Some(command_receiver),
            tool_refresh_sender,
            tool_refresh_receiver: Some(tool_refresh_receiver),
            next_connection_generation: Arc::new(AtomicU64::new(1)),
            progressive_discovery_instructions: None,
            reconnect_policy: None,
        }
    }

    /// Deliver elicitations, status changes and readiness to the host. Without a
    /// sender, events are dropped and server elicitations are cancelled.
    pub fn with_event_sender(mut self, sender: mpsc::Sender<McpClientEvent>) -> Self {
        self.event_sender = Some(sender);
        self
    }

    pub fn with_client_capabilities(mut self, capabilities: ClientCapabilities) -> Self {
        self.client_info.capabilities = capabilities;
        self
    }

    pub fn with_client_info(mut self, client_info: Implementation) -> Self {
        self.client_info.client_info = client_info;
        self
    }

    pub fn with_progressive_discovery_instructions(mut self, instructions: impl Into<String>) -> Self {
        self.progressive_discovery_instructions = Some(instructions.into());
        self
    }

    #[cfg(feature = "oauth")]
    pub fn with_oauth_handler_factory(mut self, factory: OAuthHandlerFactory) -> Self {
        self.oauth_handler_factory = Some(factory);
        self
    }

    #[cfg(feature = "oauth")]
    pub fn with_oauth_credential_store(mut self, store: Arc<dyn OAuthCredentialStorage>) -> Self {
        self.oauth_credential_store = Some(store);
        self
    }

    pub fn with_root_dir(mut self, root_dir: impl Into<PathBuf>) -> Self {
        self.root_dir = root_dir.into();
        self
    }

    pub fn with_tool_filter(mut self, filter: ToolFilter) -> Self {
        self.tool_filter = filter;
        self
    }

    /// Retry HTTP servers whose connection attempt failed. Only applies to [`Self::spawn`].
    pub fn with_reconnect(mut self, policy: ReconnectPolicy) -> Self {
        self.reconnect_policy = Some(policy);
        self
    }

    /// A cloneable handle for reading snapshots and calling tools once this manager is spawned.
    pub fn handle(&self) -> McpHandle {
        McpHandle::new(self.command_sender.clone(), self.snapshot_sender.subscribe())
    }

    /// Connect `servers` in the background. The returned task applies connection
    /// attempts, tool list changes and reconnects until every [`McpHandle`] and
    /// snapshot subscription is dropped, then shuts the servers down.
    pub async fn spawn(mut self, servers: Vec<RuntimeMcpServer>) -> Result<JoinHandle<()>> {
        let pending = self.register_pending(servers).await?;
        Ok(tokio::spawn(manager_task::run(self, pending)))
    }

    pub fn catalog(&self) -> &ToolCatalog {
        &self.catalog
    }

    pub async fn register_pending(&mut self, servers: Vec<RuntimeMcpServer>) -> Result<Vec<RuntimeMcpServer>> {
        if let Some(server) = servers.iter().find(|server| server.name.contains(SERVERNAME_DELIMITER)) {
            return Err(McpError::InvalidServerName(server.name.clone()));
        }
        let reconnect = self.reconnect_policy.is_some();
        for server in &servers {
            let record =
                self.register_record(&server.name, ServerState::Connecting, None, server.tool_exposure.clone());
            if reconnect && let RuntimeMcpTransport::Http(config) = &server.transport {
                record.reconnect = Some(ReconnectState { config: config.clone(), failures: 0 });
            }
        }

        self.publish_snapshot();
        self.emit_server_statuses_changed().await;
        Ok(servers)
    }

    pub fn connect_pending_task(
        &self,
        server: RuntimeMcpServer,
    ) -> impl Future<Output = McpConnectAttempt> + Send + 'static {
        let ctx = self.connect_config();
        async move { connect_server(server, &ctx).await }
    }

    pub async fn add_mcps(&mut self, servers: Vec<RuntimeMcpServer>) -> Result<()> {
        let pending = self.register_pending(servers).await?;
        let ctx = self.connect_config();
        let attempts = join_all(pending.into_iter().map(|server| connect_server(server, &ctx))).await;
        for attempt in attempts {
            self.apply_connection_attempt(attempt).await;
        }
        Ok(())
    }

    pub fn server_instructions(&self) -> BTreeMap<String, String> {
        self.catalog.model_instructions()
    }

    pub fn server_statuses(&self) -> Vec<McpServerStatusEntry> {
        self.catalog.server_statuses()
    }

    #[cfg(feature = "oauth")]
    pub async fn authenticate_server_task(
        &mut self,
        name: &str,
    ) -> Result<impl Future<Output = McpConnectAttempt> + Send + 'static> {
        let record = self
            .servers
            .get(name)
            .ok_or_else(|| McpError::ConnectionFailed(format!("server '{name}' is not OAuth-authenticatable")))?;
        if !record.can_authenticate() {
            return Err(McpError::ConnectionFailed(format!("server '{name}' is not OAuth-authenticatable")));
        }
        if self.oauth_handler_factory.is_none() {
            return Err(McpError::ConnectionFailed(format!("No OAuth handler factory available for '{name}'")));
        }

        let name = name.to_string();
        let config = record.reauth_config.clone().expect("checked above");
        let challenge = record.oauth_challenge.clone();
        let ctx = self.connect_config();

        self.set_state(&name, ServerState::Authenticating);
        self.emit_server_statuses_changed().await;

        Ok(async move { authenticate_http(name, config, challenge, ctx).await })
    }

    pub async fn apply_connection_attempt(&mut self, attempt: McpConnectAttempt) {
        let McpConnectAttempt { name, outcome } = attempt;
        match outcome {
            McpConnectOutcome::Connected { conn, reauth_config } => {
                match self.register_connection(&name, conn, reauth_config).await {
                    Ok(()) => {
                        self.emit_server_statuses_changed().await;
                    }
                    Err(error) => self.apply_authentication_failure(name, error.to_string()).await,
                }
            }
            McpConnectOutcome::NeedsOAuth { config, challenge, error } => {
                tracing::warn!("Server '{name}' needs OAuth: {error}");
                if let Some(record) = self.servers.get_mut(&name) {
                    record.reauth_config = Some(config);
                    record.oauth_challenge = challenge;
                }
                self.set_state(&name, ServerState::NeedsOAuth);
                self.emit_server_statuses_changed().await;
            }
            McpConnectOutcome::Failed { error } => {
                self.apply_authentication_failure(name, error.to_string()).await;
            }
        }
    }

    /// List all prompts from all connected MCP servers with namespacing
    pub async fn list_prompts(&self) -> Result<Vec<rmcp::model::Prompt>> {
        let futures: Vec<_> = self
            .servers
            .iter()
            .filter_map(|(server_name, record)| {
                let conn = record.connection()?;
                conn.client.peer_info()?.capabilities.prompts.as_ref()?;
                let server_name = server_name.clone();
                let client = conn.client.clone();
                Some(async move {
                    let prompts = client.list_all_prompts().await.map_err(|e| {
                        McpError::PromptListFailed(format!("Failed to list prompts for {server_name}: {e}"))
                    })?;

                    let namespaced_prompts: Vec<rmcp::model::Prompt> = prompts
                        .into_iter()
                        .map(|prompt| {
                            let namespaced_name = create_namespaced_tool_name(&server_name, &prompt.name);
                            rmcp::model::Prompt::new(namespaced_name, prompt.description, prompt.arguments)
                        })
                        .collect();

                    Ok::<_, McpError>(namespaced_prompts)
                })
            })
            .collect();

        let results = join_all(futures).await;
        let mut all_prompts = Vec::new();
        for result in results {
            all_prompts.extend(result?);
        }

        Ok(all_prompts)
    }

    /// Get a specific prompt by namespaced name
    pub async fn get_prompt(
        &self,
        namespaced_prompt_name: &str,
        arguments: Option<serde_json::Map<String, serde_json::Value>>,
    ) -> Result<rmcp::model::GetPromptResult> {
        let (server_name, prompt_name) = split_on_server_name(namespaced_prompt_name)
            .ok_or_else(|| McpError::InvalidToolNameFormat(namespaced_prompt_name.to_string()))?;

        let server_conn =
            self.connection_for(server_name).ok_or_else(|| McpError::ServerNotFound(server_name.to_string()))?;

        let mut request = rmcp::model::GetPromptRequestParams::new(prompt_name);
        if let Some(args) = arguments {
            request = request.with_arguments(args);
        }

        server_conn.client.get_prompt(request).await.map_err(|e| {
            McpError::PromptGetFailed(format!("Failed to get prompt '{prompt_name}' from {server_name}: {e}"))
        })
    }

    /// Shutdown all servers and wait for their tasks to complete
    pub async fn shutdown(&mut self) {
        let servers: Vec<(String, ServerRecord)> = self.servers.drain().collect();
        self.catalog = ToolCatalog::new();
        self.publish_snapshot();

        for (server_name, record) in servers {
            if let Some(conn) = record.into_connection()
                && let Some(handle) = conn.server_task
            {
                drop(conn.client);
                await_server_shutdown(&server_name, handle).await;
            }
        }
    }

    /// Shutdown a specific server by name
    pub async fn shutdown_server(&mut self, server_name: &str) -> Result<()> {
        self.catalog.remove_server(server_name);
        self.refresh_progressive_discovery_instructions();
        let record = self.servers.remove(server_name);
        self.publish_snapshot();
        if let Some(record) = record
            && let Some(conn) = record.into_connection()
            && let Some(handle) = conn.server_task
        {
            drop(conn.client);
            await_server_shutdown(server_name, handle).await;
        }

        Ok(())
    }

    pub async fn emit_connection_ready(&self) {
        self.emit_event(McpClientEvent::ConnectionReady(self.snapshot())).await;
    }

    pub fn snapshot(&self) -> Arc<McpSnapshot> {
        let clients = self
            .servers
            .iter()
            .filter_map(|(name, record)| record.connection().map(|conn| (name.clone(), conn.client.clone())))
            .collect();
        Arc::new(McpSnapshot::new(Arc::new(self.catalog.clone()), Arc::new(clients)))
    }

    pub(crate) async fn apply_tool_list_refresh(&mut self, refresh: ToolListRefresh) {
        let ToolListRefresh { server, generation, result } = refresh;
        let Some(record) = self.servers.get(&server) else {
            return;
        };
        let Some(connection) = record.connection() else {
            return;
        };
        if connection.generation() != generation {
            tracing::debug!(server = %server, generation, "Ignoring stale MCP tool refresh");
            return;
        }
        let tools = match result {
            Ok(tools) => tools,
            Err(error) => {
                tracing::warn!(server = %server, %error, "Failed to refresh MCP tools; retaining previous catalog");
                return;
            }
        };
        if let Err(error) = self.replace_catalog_tools(&server, &tools) {
            tracing::warn!(server = %server, %error, "Failed to apply refreshed MCP tools; retaining previous catalog");
            return;
        }
        self.emit_server_statuses_changed().await;
    }

    /// A delayed retry for `name` if it failed and the reconnect policy covers it.
    pub(crate) fn reconnect_task(
        &mut self,
        name: &str,
    ) -> Option<impl Future<Output = McpConnectAttempt> + Send + 'static> {
        let policy = self.reconnect_policy?;
        let record = self.servers.get_mut(name)?;
        let failed = matches!(record.state, ServerState::Failed { .. });
        let reconnect = record.reconnect.as_mut()?;
        if !failed {
            reconnect.failures = 0;
            return None;
        }
        let delay = policy.delay(reconnect.failures);
        reconnect.failures = reconnect.failures.saturating_add(1);
        let transport = RuntimeMcpTransport::Http(reconnect.config.clone());
        let exposure = self.catalog.server(name).map_or(ToolExposure::ModelVisible, |entry| entry.exposure().clone());
        let server = RuntimeMcpServer::new(name, transport, exposure);
        let ctx = self.connect_config();
        Some(async move {
            tokio::time::sleep(delay).await;
            connect_server(server, &ctx).await
        })
    }

    pub(crate) fn take_command_receiver(&mut self) -> mpsc::Receiver<ManagerCommand> {
        self.command_receiver.take().expect("command receiver can only be taken once")
    }

    pub(crate) fn take_tool_refresh_receiver(&mut self) -> mpsc::Receiver<ToolListChangedRequest> {
        self.tool_refresh_receiver.take().expect("tool refresh receiver can only be taken once")
    }

    pub(crate) fn snapshot_sender(&self) -> watch::Sender<Arc<McpSnapshot>> {
        self.snapshot_sender.clone()
    }

    async fn emit_server_statuses_changed(&self) {
        self.emit_event(McpClientEvent::ServerStatusesChanged(self.server_statuses())).await;
    }

    fn refresh_progressive_discovery_instructions(&mut self) {
        let instructions = (!self.catalog.discoverable_deferred_servers().is_empty())
            .then(|| self.progressive_discovery_instructions.clone())
            .flatten();
        self.catalog.set_progressive_discovery_instructions(instructions);
    }

    async fn emit_authentication_failed(&self, server: String, error: String) {
        self.emit_event(McpClientEvent::AuthenticationFailed { server, error }).await;
    }

    async fn emit_event(&self, event: McpClientEvent) {
        if let Some(sender) = &self.event_sender
            && let Err(e) = sender.send(event).await
        {
            tracing::warn!("Failed to emit MCP client event: {e}");
        }
    }

    fn connect_config(&self) -> Arc<ConnectConfig> {
        Arc::new(ConnectConfig {
            client_info: self.client_info.clone(),
            event_sender: self.event_sender.clone(),
            tool_refresh_sender: self.tool_refresh_sender.clone(),
            next_connection_generation: Arc::clone(&self.next_connection_generation),
            root_dir: self.root_dir.clone(),
            #[cfg(feature = "oauth")]
            oauth_handler_factory: self.oauth_handler_factory.clone(),
            #[cfg(feature = "oauth")]
            oauth_credential_store: self.oauth_credential_store.clone(),
        })
    }

    async fn register_connection(
        &mut self,
        name: &str,
        conn: McpServerConnection,
        reauth_config: Option<McpHttpConfig>,
    ) -> Result<()> {
        let tools = conn
            .list_tools()
            .await
            .map_err(|e| McpError::ToolDiscoveryFailed(format!("Failed to list tools for {name}: {e}")))?;
        let exposure =
            self.catalog.server(name).ok_or_else(|| McpError::ServerNotFound(name.to_string()))?.exposure().clone();

        let auth_capability = {
            let record = self.servers.get_mut(name).expect("record checked above");
            record.reauth_config = reauth_config.or_else(|| record.reauth_config.clone());
            record.oauth_challenge = None;
            record.auth_capability()
        };
        let description = conn
            .client
            .peer_info()
            .and_then(|info| info.server_info.as_ref()?.description.clone())
            .filter(|description| !description.is_empty())
            .unwrap_or_else(|| name.to_string());
        let instructions = conn.instructions.clone();
        let entry = ServerCatalogEntry::new(
            name,
            description,
            instructions,
            McpServerStatus::Connected { tool_count: tools.len() },
            auth_capability,
            exposure,
            &tools,
            &self.tool_filter,
        );

        self.servers.get_mut(name).expect("record checked above").state = ServerState::Connected { connection: conn };
        self.catalog.upsert_server(entry);
        self.refresh_progressive_discovery_instructions();
        self.publish_snapshot();
        Ok(())
    }

    fn replace_catalog_tools(&mut self, name: &str, tools: &[RmcpTool]) -> Result<()> {
        let existing = self.catalog.server(name).cloned().ok_or_else(|| McpError::ServerNotFound(name.to_string()))?;
        let entry = ServerCatalogEntry::new(
            name,
            existing.description(),
            existing.instructions().map(str::to_string),
            McpServerStatus::Connected { tool_count: tools.len() },
            existing.auth_capability(),
            existing.exposure().clone(),
            tools,
            &self.tool_filter,
        );
        self.catalog.upsert_server(entry);
        self.refresh_progressive_discovery_instructions();
        self.publish_snapshot();
        Ok(())
    }

    async fn apply_authentication_failure(&mut self, name: String, error: String) {
        self.set_state(&name, ServerState::Failed { error: error.clone() });
        self.emit_server_statuses_changed().await;
        self.emit_authentication_failed(name, error).await;
    }

    fn set_state(&mut self, name: &str, state: ServerState) {
        let status = McpServerStatus::from(&state);
        match self.servers.get_mut(name) {
            Some(record) => record.state = state,
            None => {
                self.servers.insert(name.to_string(), ServerRecord::new(state, None));
            }
        }
        let auth = self.servers.get(name).map_or(McpServerAuthCapability::Unavailable, ServerRecord::auth_capability);
        let entry = self
            .catalog
            .server(name)
            .cloned()
            .unwrap_or_else(|| ServerCatalogEntry::pending(name, ToolExposure::ModelVisible))
            .with_status(status, auth);
        self.catalog.upsert_server(entry);
        self.refresh_progressive_discovery_instructions();
        self.publish_snapshot();
    }

    fn register_record(
        &mut self,
        name: &str,
        state: ServerState,
        reauth_config: Option<McpHttpConfig>,
        exposure: ToolExposure,
    ) -> &mut ServerRecord {
        let status = McpServerStatus::from(&state);
        let auth_capability =
            if reauth_config.is_some() { McpServerAuthCapability::OAuth } else { McpServerAuthCapability::Unavailable };
        self.catalog.upsert_server(ServerCatalogEntry::pending(name, exposure).with_status(status, auth_capability));
        self.servers.entry(name.to_string()).insert_entry(ServerRecord::new(state, reauth_config)).into_mut()
    }

    fn publish_snapshot(&self) {
        self.snapshot_sender.send_replace(self.snapshot());
    }

    fn connection_for(&self, server_name: &str) -> Option<&McpServerConnection> {
        self.servers.get(server_name).and_then(ServerRecord::connection)
    }
}

impl Drop for McpManager {
    fn drop(&mut self) {
        let servers: Vec<(String, ServerRecord)> = self.servers.drain().collect();
        for (server_name, record) in servers {
            if let Some(conn) = record.into_connection()
                && let Some(handle) = conn.server_task
            {
                handle.abort();
                tracing::warn!("Server '{server_name}' task aborted during cleanup");
            }
        }
    }
}

pub(crate) struct ToolListChangedRequest {
    server: String,
    generation: u64,
    peer: Peer<RoleClient>,
}

pub(crate) struct ToolListRefresh {
    server: String,
    generation: u64,
    result: Result<Vec<RmcpTool>>,
}

impl ToolListChangedRequest {
    pub(crate) fn new(server: String, generation: u64, peer: Peer<RoleClient>) -> Self {
        Self { server, generation, peer }
    }

    pub(crate) async fn refresh(self) -> ToolListRefresh {
        let result = self
            .peer
            .list_all_tools()
            .await
            .map_err(|error| McpError::ToolDiscoveryFailed(format!("Failed to refresh tools: {error}")));
        ToolListRefresh { server: self.server, generation: self.generation, result }
    }
}

/// Internal record holding all mutable state for a single MCP server.
struct ServerRecord {
    state: ServerState,
    reauth_config: Option<McpHttpConfig>,
    oauth_challenge: Option<String>,
    reconnect: Option<ReconnectState>,
}

struct ReconnectState {
    config: McpHttpConfig,
    failures: u32,
}

enum ServerState {
    Connecting,
    Connected {
        connection: McpServerConnection,
    },
    #[cfg_attr(not(feature = "oauth"), allow(dead_code))]
    Authenticating,
    Failed {
        error: String,
    },
    NeedsOAuth,
}

impl From<&ServerState> for McpServerStatus {
    fn from(state: &ServerState) -> Self {
        match state {
            ServerState::Connecting => Self::Connecting,
            ServerState::Connected { .. } => Self::Connected { tool_count: 0 },
            ServerState::Authenticating => Self::Authenticating,
            ServerState::Failed { error } => Self::Failed { error: error.clone() },
            ServerState::NeedsOAuth => Self::NeedsOAuth,
        }
    }
}

impl ServerRecord {
    fn new(state: ServerState, reauth_config: Option<McpHttpConfig>) -> Self {
        Self { state, reauth_config, oauth_challenge: None, reconnect: None }
    }

    fn connection(&self) -> Option<&McpServerConnection> {
        match &self.state {
            ServerState::Connected { connection, .. } => Some(connection),
            ServerState::Connecting
            | ServerState::Authenticating
            | ServerState::Failed { .. }
            | ServerState::NeedsOAuth => None,
        }
    }

    fn into_connection(self) -> Option<McpServerConnection> {
        match self.state {
            ServerState::Connected { connection, .. } => Some(connection),
            ServerState::Connecting
            | ServerState::Authenticating
            | ServerState::Failed { .. }
            | ServerState::NeedsOAuth => None,
        }
    }

    fn auth_capability(&self) -> McpServerAuthCapability {
        if self.reauth_config.is_some() { McpServerAuthCapability::OAuth } else { McpServerAuthCapability::Unavailable }
    }

    #[cfg(feature = "oauth")]
    fn can_authenticate(&self) -> bool {
        self.reauth_config.is_some()
    }
}

/// Awaits `handle` for up to 5 seconds, logging whether the server shut down
/// gracefully, panicked, or timed out. Used during manager teardown.
async fn await_server_shutdown(server_name: &str, handle: JoinHandle<()>) {
    let Ok(task_result) = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await else {
        tracing::warn!("Server '{server_name}' shutdown timed out");
        return;
    };
    match task_result {
        Ok(()) => tracing::info!("Server '{server_name}' shut down gracefully"),
        Err(e) => tracing::warn!("Server '{server_name}' task panicked: {e:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        McpClientEvent, McpManager, McpServerStatus, RuntimeMcpServer as McpServer,
        RuntimeMcpTransport as McpTransport, ServerState, ToolListRefresh,
    };
    use crate::client::config::{McpHttpConfig, ToolExposure};
    use crate::client::connection::{McpConnectAttempt, McpConnectOutcome};
    use crate::client::{McpSnapshot, ToolRoute};
    use rmcp::{
        Json, RoleServer, ServerHandler,
        handler::server::{router::tool::ToolRouter, wrapper::Parameters},
        model::{Implementation, ServerCapabilities, ServerConfig, Tool as RmcpTool},
        service::DynService,
        tool, tool_handler, tool_router,
        transport::streamable_http_client::StreamableHttpClientTransportConfig,
    };
    use schemars::JsonSchema;
    use serde::{Deserialize, Serialize};
    use std::{
        io,
        sync::{Arc, Mutex},
    };
    use tokio::sync::mpsc;
    use utils::mcp_status::McpServerAuthCapability;

    #[derive(Clone)]
    struct TestServer {
        tool_router: ToolRouter<Self>,
    }

    #[allow(clippy::unused_async_trait_impl)]
    #[tool_handler(router = self.tool_router)]
    impl ServerHandler for TestServer {
        fn get_info(&self) -> ServerConfig {
            ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
                .with_server_info(Implementation::new("test-server", "0.1.0").with_description("Test MCP server"))
                .with_instructions("Test server instructions")
        }
    }

    impl Default for TestServer {
        fn default() -> Self {
            Self { tool_router: Self::tool_router() }
        }
    }

    #[derive(Debug, Deserialize, Serialize, JsonSchema)]
    struct EchoRequest {
        value: String,
    }

    #[derive(Debug, Deserialize, Serialize, JsonSchema)]
    struct EchoResult {
        value: String,
    }

    #[tool_router]
    impl TestServer {
        fn into_dyn(self) -> Box<dyn DynService<RoleServer>> {
            Box::new(self)
        }

        #[tool(description = "Returns the provided value", annotations(read_only_hint = true, open_world_hint = false))]
        async fn echo(&self, request: Parameters<EchoRequest>) -> Json<EchoResult> {
            let Parameters(EchoRequest { value }) = request;
            Json(EchoResult { value })
        }
    }

    #[derive(Clone)]
    struct SharedWriter(Arc<Mutex<Vec<u8>>>);

    impl io::Write for SharedWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn register_pending_marks_every_server_connecting_and_emits_status() {
        let (event_sender, mut event_receiver) = mpsc::channel(32);
        let mut manager = McpManager::new().with_event_sender(event_sender);

        let servers = vec![
            McpServer::new(
                "alpha",
                McpTransport::InMemory { server: TestServer::default().into_dyn() },
                ToolExposure::ModelVisible,
            ),
            McpServer::new(
                "beta",
                McpTransport::InMemory { server: TestServer::default().into_dyn() },
                ToolExposure::deferred_all(),
            ),
        ];

        let returned = manager.register_pending(servers).await.unwrap();
        assert_eq!(returned.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), vec!["alpha", "beta"]);

        let statuses = manager.server_statuses();
        assert_eq!(statuses.len(), 2);
        assert!(matches!(statuses.iter().find(|s| s.name == "alpha").unwrap().status, McpServerStatus::Connecting));
        assert!(matches!(statuses.iter().find(|s| s.name == "beta").unwrap().status, McpServerStatus::Connecting));
        assert!(statuses.iter().find(|s| s.name == "beta").unwrap().deferred_tools);

        let event = event_receiver.try_recv().expect("expected initial ServerStatusesChanged emission");
        let McpClientEvent::ServerStatusesChanged(emitted) = event else {
            panic!("expected ServerStatusesChanged, got {event:?}");
        };
        assert_eq!(emitted.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), vec!["alpha", "beta"]);
    }

    #[tokio::test]
    async fn server_statuses_mark_model_visible_and_deferred_servers() {
        let (event_sender, _event_receiver) = mpsc::channel(32);
        let mut manager = McpManager::new().with_event_sender(event_sender);
        manager
            .add_mcps(vec![
                McpServer::new(
                    "direct",
                    McpTransport::InMemory { server: TestServer::default().into_dyn() },
                    ToolExposure::ModelVisible,
                ),
                McpServer::new(
                    "math",
                    McpTransport::InMemory { server: TestServer::default().into_dyn() },
                    ToolExposure::deferred_all(),
                ),
            ])
            .await
            .unwrap();

        let statuses = manager.server_statuses();
        assert_eq!(statuses.iter().map(|status| status.name.as_str()).collect::<Vec<_>>(), vec!["direct", "math"]);
        assert!(!statuses.iter().find(|status| status.name == "direct").unwrap().deferred_tools);
        assert!(statuses.iter().find(|status| status.name == "math").unwrap().deferred_tools);
    }

    #[tokio::test]
    async fn tools_drop_when_a_server_shuts_down() {
        let (event_sender, _event_receiver) = mpsc::channel(32);
        let mut manager = McpManager::new().with_event_sender(event_sender);
        manager
            .add_mcps(vec![
                McpServer::new(
                    "git",
                    McpTransport::InMemory { server: TestServer::default().into_dyn() },
                    ToolExposure::ModelVisible,
                ),
                McpServer::new(
                    "github",
                    McpTransport::InMemory { server: TestServer::default().into_dyn() },
                    ToolExposure::ModelVisible,
                ),
            ])
            .await
            .unwrap();

        assert!(visible_tool_names(&manager.snapshot()).contains(&"git__echo".to_string()));
        assert!(visible_tool_names(&manager.snapshot()).contains(&"github__echo".to_string()));

        manager.shutdown_server("git").await.unwrap();

        assert!(!visible_tool_names(&manager.snapshot()).iter().any(|name| name.starts_with("git__")));
        assert!(visible_tool_names(&manager.snapshot()).contains(&"github__echo".to_string()));
    }

    #[tokio::test]
    async fn server_removal_publishes_before_stale_connection_cleanup() {
        let (event_sender, _event_receiver) = mpsc::channel(32);
        let mut manager = McpManager::new().with_event_sender(event_sender);
        let mut snapshots = manager.handle().subscribe();
        manager
            .add_mcps(vec![McpServer::new(
                "test",
                McpTransport::InMemory { server: TestServer::default().into_dyn() },
                ToolExposure::ModelVisible,
            )])
            .await
            .unwrap();
        let connected = snapshots.borrow_and_update().clone();
        assert_eq!(visible_tool_names(&connected), ["test__echo"]);

        manager.shutdown_server("test").await.unwrap();
        snapshots.changed().await.unwrap();
        let removed = snapshots.borrow().clone();

        assert!(visible_tool_names(&removed).is_empty());
        assert!(
            removed
                .resolve(ToolRoute::ModelVisible { namespaced_name: "test__echo".to_string() }, serde_json::Map::new(),)
                .is_err()
        );
        assert_eq!(visible_tool_names(&connected), ["test__echo"]);
    }

    #[tokio::test]
    async fn failed_tool_refresh_preserves_last_healthy_snapshot() {
        let (event_sender, _event_receiver) = mpsc::channel(32);
        let mut manager = McpManager::new().with_event_sender(event_sender);
        let snapshots = manager.handle().subscribe();
        manager
            .add_mcps(vec![McpServer::new(
                "test",
                McpTransport::InMemory { server: TestServer::default().into_dyn() },
                ToolExposure::ModelVisible,
            )])
            .await
            .unwrap();
        let healthy = snapshots.borrow().clone();
        let generation = manager.connection_for("test").unwrap().generation();

        manager
            .apply_tool_list_refresh(ToolListRefresh {
                server: "test".to_string(),
                generation,
                result: Err(crate::client::McpError::ToolDiscoveryFailed("boom".to_string())),
            })
            .await;

        assert!(Arc::ptr_eq(&healthy, &snapshots.borrow()));
        assert_eq!(visible_tool_names(&snapshots.borrow()), ["test__echo"]);
    }

    #[tokio::test]
    async fn stale_tool_refresh_is_ignored_after_connection_generation_changes() {
        let (event_sender, _event_receiver) = mpsc::channel(32);
        let mut manager = McpManager::new().with_event_sender(event_sender);
        let snapshots = manager.handle().subscribe();
        manager
            .add_mcps(vec![McpServer::new(
                "test",
                McpTransport::InMemory { server: TestServer::default().into_dyn() },
                ToolExposure::ModelVisible,
            )])
            .await
            .unwrap();
        let healthy = snapshots.borrow().clone();
        let stale_generation = manager.connection_for("test").unwrap().generation() + 1;
        let added = RmcpTool::new("stale", "stale", Arc::new(serde_json::Map::new()));

        manager
            .apply_tool_list_refresh(ToolListRefresh {
                server: "test".to_string(),
                generation: stale_generation,
                result: Ok(vec![added]),
            })
            .await;

        assert!(Arc::ptr_eq(&healthy, &snapshots.borrow()));
        assert!(!visible_tool_names(&snapshots.borrow()).contains(&"test__stale".to_string()));
    }

    #[tokio::test]
    async fn tools_preserve_annotations() {
        let (event_sender, _event_receiver) = mpsc::channel(32);
        let mut manager = McpManager::new().with_event_sender(event_sender);
        manager
            .add_mcps(vec![McpServer::new(
                "test",
                McpTransport::InMemory { server: TestServer::default().into_dyn() },
                ToolExposure::ModelVisible,
            )])
            .await
            .unwrap();

        let echo = manager.catalog().tool("test__echo").expect("echo tool").tool();
        let annotations = echo.annotations.as_ref().expect("annotations should be preserved");
        assert_eq!(annotations.read_only_hint, Some(true));
        assert_eq!(annotations.open_world_hint, Some(false));
    }

    #[tokio::test]
    async fn drop_logs_cleanup_abort_with_tracing() {
        let (event_sender, _event_receiver) = mpsc::channel(32);
        let mut manager = McpManager::new().with_event_sender(event_sender);
        manager
            .add_mcps(vec![McpServer::new(
                "test",
                McpTransport::InMemory { server: TestServer::default().into_dyn() },
                ToolExposure::ModelVisible,
            )])
            .await
            .unwrap();

        let output = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_writer({
                let output = Arc::clone(&output);
                move || SharedWriter(Arc::clone(&output))
            })
            .finish();

        tracing::subscriber::with_default(subscriber, || {
            drop(manager);
        });

        let logs = String::from_utf8(output.lock().unwrap().clone()).unwrap();
        assert!(logs.contains("Server 'test' task aborted during cleanup"));
    }

    fn visible_tool_names(snapshot: &McpSnapshot) -> Vec<String> {
        snapshot.catalog().tools().model_visible.iter().map(|tool| tool.namespaced_name().to_string()).collect()
    }

    #[cfg(feature = "oauth")]
    mod oauth {
        use super::*;
        use crate::client::OAuthHandlerFactory;
        use aether_auth::{OAuthError, OAuthHandler};
        use futures::future::BoxFuture;

        struct TestOAuthHandler;

        impl OAuthHandler for TestOAuthHandler {
            fn redirect_uri(&self) -> &'static str {
                "http://127.0.0.1:0/oauth2callback"
            }

            fn authorize(&self, _auth_url: &str) -> BoxFuture<'_, Result<String, OAuthError>> {
                Box::pin(async { Err(OAuthError::UserCancelled) })
            }
        }

        fn test_oauth_handler_factory() -> OAuthHandlerFactory {
            Arc::new(|_ctx| Ok(Arc::new(TestOAuthHandler)))
        }

        fn http_config(uri: &str) -> McpHttpConfig {
            StreamableHttpClientTransportConfig::with_uri(uri).into()
        }

        #[tokio::test]
        async fn authenticate_server_task_rejects_record_without_reauth_config() {
            let (event_sender, _event_receiver) = mpsc::channel(1);
            let mut manager = McpManager::new()
                .with_event_sender(event_sender)
                .with_oauth_handler_factory(test_oauth_handler_factory());
            manager.register_record("public", ServerState::Connecting, None, ToolExposure::ModelVisible);

            let error = match manager.authenticate_server_task("public").await {
                Ok(_) => panic!("non-OAuth server should be rejected"),
                Err(error) => error.to_string(),
            };
            assert!(error.contains("not OAuth-authenticatable"));
        }

        #[tokio::test]
        async fn authenticate_server_task_marks_server_authenticating_and_emits_status() {
            let (event_sender, mut event_receiver) = mpsc::channel(2);
            let mut manager = McpManager::new()
                .with_event_sender(event_sender)
                .with_oauth_handler_factory(test_oauth_handler_factory());
            manager.register_record(
                "remote",
                ServerState::NeedsOAuth,
                Some(http_config("http://localhost:19999/mcp")),
                ToolExposure::ModelVisible,
            );

            let _task = manager.authenticate_server_task("remote").await.expect("auth should start");

            assert!(matches!(manager.server_statuses()[0].status, McpServerStatus::Authenticating));
            let event = event_receiver.recv().await.expect("status change event");
            let McpClientEvent::ServerStatusesChanged(servers) = event else {
                panic!("expected ServerStatusesChanged");
            };
            let status = servers.iter().find(|entry| entry.name == "remote").expect("remote status");
            assert!(matches!(status.status, McpServerStatus::Authenticating));
            assert_eq!(status.auth_capability, McpServerAuthCapability::OAuth);
        }

        #[tokio::test]
        async fn apply_connection_attempt_failure_allows_retry() {
            let (event_sender, mut event_receiver) = mpsc::channel(2);
            let mut manager = McpManager::new()
                .with_event_sender(event_sender)
                .with_oauth_handler_factory(test_oauth_handler_factory());
            manager.register_record(
                "remote",
                ServerState::NeedsOAuth,
                Some(http_config("http://localhost:19999/mcp")),
                ToolExposure::ModelVisible,
            );
            let _task = manager.authenticate_server_task("remote").await.expect("auth should start");
            let _authenticating_event = event_receiver.recv().await.expect("authenticating status change event");

            manager
                .apply_connection_attempt(McpConnectAttempt {
                    name: "remote".to_string(),
                    outcome: McpConnectOutcome::Failed {
                        error: crate::client::McpError::ConnectionFailed("boom".to_string()),
                    },
                })
                .await;

            let event = event_receiver.recv().await.expect("status change event");
            let McpClientEvent::ServerStatusesChanged(servers) = event else {
                panic!("expected ServerStatusesChanged");
            };
            let auth_event = event_receiver.recv().await.expect("authentication failure event");
            let McpClientEvent::AuthenticationFailed { server, error } = auth_event else {
                panic!("expected AuthenticationFailed");
            };
            assert_eq!(server, "remote");
            assert!(error.contains("boom"));

            let status = servers.iter().find(|entry| entry.name == "remote").expect("remote status");
            assert_eq!(status.auth_capability, McpServerAuthCapability::OAuth);
            assert!(matches!(status.status, McpServerStatus::Failed { ref error } if error.contains("boom")));
            assert!(manager.authenticate_server_task("remote").await.is_ok());
        }

        #[test]
        fn status_entries_are_derived_from_reauth_config() {
            let (event_sender, _event_receiver) = mpsc::channel(1);
            let mut manager = McpManager::new()
                .with_event_sender(event_sender)
                .with_oauth_handler_factory(test_oauth_handler_factory());

            manager.register_record(
                "with-oauth",
                ServerState::Connecting,
                Some(http_config("http://localhost/mcp")),
                ToolExposure::ModelVisible,
            );
            manager.register_record("without-oauth", ServerState::Connecting, None, ToolExposure::ModelVisible);
            manager.register_record(
                "needs-oauth",
                ServerState::NeedsOAuth,
                Some(http_config("http://localhost/mcp2")),
                ToolExposure::ModelVisible,
            );

            let statuses = manager.server_statuses();
            let with_oauth = statuses.iter().find(|s| s.name == "with-oauth").unwrap();
            let without_oauth = statuses.iter().find(|s| s.name == "without-oauth").unwrap();
            let needs_oauth = statuses.iter().find(|s| s.name == "needs-oauth").unwrap();

            assert_eq!(with_oauth.auth_capability, McpServerAuthCapability::OAuth);
            assert_eq!(without_oauth.auth_capability, McpServerAuthCapability::Unavailable);
            assert_eq!(needs_oauth.auth_capability, McpServerAuthCapability::OAuth);
        }
    }
}
