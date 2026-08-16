use llm::ToolDefinition;

use super::{
    McpError, Result,
    config::{McpHttpConfig, McpServer, ToolExposure},
    connection::{
        ConnectConfig, McpConnectAttempt, McpConnectOutcome, McpServerConnection, Tool, authenticate_http,
        connect_server,
    },
    mcp_client::{McpClient, client_capabilities},
    naming::{create_namespaced_tool_name, split_on_server_name},
    tool_filter::ToolFilter,
};
use crate::tool_gateway::ServerDescription;
use aether_auth::{OAuthCredentialStorage, OAuthHandler};
use futures::future::join_all;
use rmcp::{
    RoleClient,
    model::{CallToolRequestParams, ClientInfo, ElicitRequestParams, ElicitResult, ElicitationAction, Implementation},
    service::RunningService,
};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::num::NonZeroU16;
use std::path::PathBuf;
use std::sync::Arc;

pub type ProgressiveDiscoveryInstructions = Arc<dyn Fn(&[ServerDescription]) -> String + Send + Sync>;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

pub use crate::status::{McpServerAuthCapability, McpServerStatus, McpServerStatusEntry};

pub const PROGRESSIVE_DISCOVERY_INSTRUCTION_NAME: &str = "progressive-discovery";

pub type OAuthHandlerFactory = Arc<dyn Fn(OAuthHandlerContext) -> Result<Arc<dyn OAuthHandler>> + Send + Sync>;

/// Context passed to an `OAuthHandlerFactory` so the constructed handler can
/// dispatch user-facing prompts back to the host through the MCP event channel.
#[derive(Clone)]
pub struct OAuthHandlerContext {
    pub server_name: String,
    pub callback_port: Option<NonZeroU16>,
    pub tx: mpsc::Sender<McpClientEvent>,
}

#[derive(Debug)]
pub struct ElicitationRequest {
    pub server_name: String,
    pub request: ElicitRequestParams,
    pub response_sender: oneshot::Sender<ElicitResult>,
}

#[derive(Debug, Clone)]
pub struct ElicitationResponse {
    pub action: ElicitationAction,
    pub content: Option<Value>,
}

/// Events emitted by MCP clients that require attention from the host
/// (e.g. the relay or TUI). Flows through a single channel from `McpManager`
/// to the consumer.
#[derive(Debug)]
pub enum McpClientEvent {
    Elicitation(ElicitationRequest),
    ServerStatusesChanged(Vec<McpServerStatusEntry>),
    ToolDefinitionsChanged(Vec<ToolDefinition>),
    ServerInstructionsUpdated { server: String, instructions: Option<String> },
    AuthenticationFailed { server: String, error: String },
    ConnectionReady(McpConnectionDetails),
}

#[derive(Debug, Clone)]
pub struct McpConnectionDetails {
    pub instructions: BTreeMap<String, String>,
    pub tool_definitions: Vec<ToolDefinition>,
    pub server_statuses: Vec<McpServerStatusEntry>,
}

#[derive(Debug)]
pub enum McpManagerEvent {
    ToolCatalogChanged { server: String, result: std::result::Result<Vec<Tool>, String> },
}

/// Manages connections to multiple MCP servers and their tools
pub struct McpManager {
    servers: HashMap<String, ServerRecord>,
    server_order: Vec<String>,
    client_info: ClientInfo,
    event_sender: mpsc::Sender<McpClientEvent>,
    manager_event_sender: mpsc::UnboundedSender<McpManagerEvent>,
    manager_event_receiver: Option<mpsc::UnboundedReceiver<McpManagerEvent>>,
    root_dir: PathBuf,
    oauth_handler_factory: Option<OAuthHandlerFactory>,
    oauth_credential_store: Option<Arc<dyn OAuthCredentialStorage>>,
    tool_filter: ToolFilter,
    progressive_discovery_instructions: Option<ProgressiveDiscoveryInstructions>,
}

impl McpManager {
    pub fn new(event_sender: mpsc::Sender<McpClientEvent>, oauth_handler_factory: Option<OAuthHandlerFactory>) -> Self {
        let (manager_event_sender, manager_event_receiver) = mpsc::unbounded_channel();
        Self {
            servers: HashMap::new(),
            server_order: Vec::new(),
            client_info: ClientInfo::new(client_capabilities(), Implementation::new("aether", "0.1.0")),
            event_sender,
            manager_event_sender,
            manager_event_receiver: Some(manager_event_receiver),
            root_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            oauth_handler_factory,
            oauth_credential_store: None,
            tool_filter: ToolFilter::default(),
            progressive_discovery_instructions: None,
        }
    }

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

    pub fn with_progressive_discovery_instructions(mut self, instructions: ProgressiveDiscoveryInstructions) -> Self {
        self.progressive_discovery_instructions = Some(instructions);
        self
    }

    pub async fn register_pending(&mut self, servers: Vec<McpServer>) -> Result<Vec<McpServer>> {
        let adds_deferred_tools = servers.iter().any(|server| server.tool_exposure.has_deferred_tools());
        let instruction_name_taken = self.servers.contains_key(PROGRESSIVE_DISCOVERY_INSTRUCTION_NAME)
            || servers.iter().any(|server| server.name == PROGRESSIVE_DISCOVERY_INSTRUCTION_NAME);
        if (adds_deferred_tools || self.has_deferred_tools()) && instruction_name_taken {
            return Err(McpError::Other(
                "server name 'progressive-discovery' collides with the progressive discovery instruction namespace"
                    .into(),
            ));
        }

        for server in &servers {
            self.register_record(&server.name, ServerState::Connecting, None, server.tool_exposure.clone());
        }

        self.emit_server_statuses_changed().await;
        Ok(servers)
    }

    pub fn connect_pending_task(&self, server: McpServer) -> impl Future<Output = McpConnectAttempt> + Send + 'static {
        let ctx = self.connect_config();
        async move { connect_server(server, &ctx).await }
    }

    pub fn take_event_receiver(&mut self) -> mpsc::UnboundedReceiver<McpManagerEvent> {
        self.manager_event_receiver.take().expect("MCP manager event receiver can only be taken once")
    }

    pub async fn apply_event(&mut self, event: McpManagerEvent) {
        match event {
            McpManagerEvent::ToolCatalogChanged { server, result: Ok(tools) } => {
                let Some(record) = self.servers.get_mut(&server) else { return };
                if !record.replace_tools(tools) {
                    return;
                }
                self.emit_server_statuses_changed().await;
                self.emit_tool_definitions_changed().await;
                self.emit_catalog_instructions_changed(&server).await;
            }
            McpManagerEvent::ToolCatalogChanged { server, result: Err(error) } => {
                tracing::warn!(server, error, "Failed to refresh MCP tool catalog");
            }
        }
    }

    pub async fn add_mcps(&mut self, servers: Vec<McpServer>) -> Result<()> {
        let pending = self.register_pending(servers).await?;
        let ctx = self.connect_config();
        let attempts = join_all(pending.into_iter().map(|server| connect_server(server, &ctx))).await;
        for attempt in attempts {
            self.apply_connection_attempt(attempt).await;
        }
        Ok(())
    }

    pub fn get_client_for_tool(
        &self,
        namespaced_tool_name: &str,
        arguments_json: &str,
    ) -> Result<(Arc<RunningService<RoleClient, McpClient>>, CallToolRequestParams)> {
        if !self.is_routable_tool(namespaced_tool_name) {
            return Err(McpError::ToolNotFound(namespaced_tool_name.to_string()));
        }

        let (server_name, tool_name) = split_on_server_name(namespaced_tool_name)
            .ok_or_else(|| McpError::InvalidToolNameFormat(namespaced_tool_name.to_string()))?;

        let client =
            self.client_for_server(server_name).ok_or_else(|| McpError::ServerNotFound(server_name.to_string()))?;

        let arguments = serde_json::from_str::<serde_json::Value>(arguments_json)?.as_object().cloned();
        let mut params = CallToolRequestParams::new(tool_name.to_string());
        if let Some(args) = arguments {
            params = params.with_arguments(args);
        }

        Ok((client, params))
    }

    pub fn deferred_servers(&self) -> Vec<ServerDescription> {
        self.server_order
            .iter()
            .filter_map(|name| {
                let record = self.servers.get(name)?;
                if record.connection().is_none()
                    || !record
                        .tools()
                        .iter()
                        .any(|tool| record.is_deferred(&tool.name) && self.is_tool_allowed(name, tool))
                {
                    return None;
                }
                Some(ServerDescription { name: name.clone(), description: record.description(name) })
            })
            .collect()
    }

    pub async fn list_deferred_tools(&mut self, server: &str) -> Result<Vec<ToolDefinition>> {
        let discovered_tools = {
            let (_, connection) = self.deferred_record(server)?;
            connection.list_tools().await?.iter().map(Tool::from).collect()
        };
        let record = self.servers.get_mut(server).ok_or_else(|| McpError::ServerNotFound(server.to_string()))?;
        let ServerState::Connected { tools, .. } = &mut record.state else {
            return Err(McpError::DeferredServerNotConnected(server.to_string()));
        };
        *tools = discovered_tools;

        let record = self.servers.get(server).expect("server was resolved above");
        Ok(record
            .tools()
            .iter()
            .filter(|tool| record.is_deferred(&tool.name) && self.is_tool_allowed(server, tool))
            .map(|tool| deferred_tool_definition(server, tool))
            .collect())
    }

    pub async fn deferred_tool(&mut self, server: &str, tool: &str) -> Result<ToolDefinition> {
        self.list_deferred_tools(server)
            .await?
            .into_iter()
            .find(|candidate| candidate.name == tool)
            .ok_or_else(|| McpError::ToolNotFound(format!("'{tool}' on server '{server}'")))
    }

    pub fn resolve_deferred_tool(
        &self,
        server: &str,
        tool: &str,
        arguments: Map<String, Value>,
    ) -> Result<(Arc<RunningService<RoleClient, McpClient>>, CallToolRequestParams)> {
        let (record, connection) = self.deferred_record(server)?;
        let Some(catalog_tool) = record.tools().iter().find(|candidate| candidate.name == tool) else {
            return Err(McpError::ToolNotFound(format!("'{tool}' on server '{server}'")));
        };
        if !record.is_deferred(tool) || !self.is_tool_allowed(server, catalog_tool) {
            return Err(McpError::ToolNotFound(format!("'{tool}' on server '{server}'")));
        }
        Ok((connection.client.clone(), CallToolRequestParams::new(tool.to_string()).with_arguments(arguments)))
    }

    pub fn progressive_discovery_instructions(&self) -> Option<String> {
        let servers = self.deferred_servers();
        (!servers.is_empty())
            .then(|| self.progressive_discovery_instructions.as_ref().map(|render| render(&servers)))
            .flatten()
    }

    pub fn has_deferred_tools(&self) -> bool {
        self.servers.values().any(|record| record.exposure.has_deferred_tools())
    }

    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        let mut definitions = Vec::new();
        for name in &self.server_order {
            let Some(record) = self.servers.get(name) else { continue };
            definitions.extend(
                record
                    .tools()
                    .iter()
                    .filter(|tool| record.is_model_visible(&tool.name) && self.is_tool_allowed(name, tool))
                    .map(|tool| namespaced_tool_definition(name, tool)),
            );
        }
        definitions
    }

    pub fn server_instructions(&self) -> BTreeMap<String, String> {
        let mut instructions: BTreeMap<String, String> = self
            .servers
            .iter()
            .filter(|(name, record)| self.has_model_visible_tools(name, record))
            .filter_map(|(name, record)| {
                record
                    .connection()
                    .and_then(|conn| conn.instructions.as_ref())
                    .map(|instr| (name.clone(), instr.clone()))
            })
            .collect();

        if let Some(body) = self.progressive_discovery_instructions() {
            instructions.insert(PROGRESSIVE_DISCOVERY_INSTRUCTION_NAME.to_string(), body);
        }

        instructions
    }

    pub fn server_statuses(&self) -> Vec<McpServerStatusEntry> {
        self.server_order
            .iter()
            .filter_map(|name| self.servers.get(name).map(|record| record.status_entry(name)))
            .collect()
    }

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
            McpConnectOutcome::Connected { conn, tools, reauth_config } => {
                match self.register_connection(&name, conn, tools, reauth_config) {
                    Ok(()) => {
                        self.emit_server_statuses_changed().await;
                        self.emit_tool_definitions_changed().await;
                        self.emit_instructions_after_connect(&name).await;
                    }
                    Err(error) => self.apply_authentication_failure(name, error.to_string()).await,
                }
            }
            McpConnectOutcome::NeedsOAuth { config, challenge, error } => {
                tracing::warn!("Server '{name}' needs OAuth: {error}");
                if let Some(record) = self.servers.get_mut(&name) {
                    record.state = ServerState::NeedsOAuth;
                    record.reauth_config = Some(config);
                    record.oauth_challenge = challenge;
                }
                self.emit_server_statuses_changed().await;
            }
            McpConnectOutcome::Failed { error } => {
                self.apply_authentication_failure(name, error.to_string()).await;
            }
        }
    }

    /// List all prompts from all connected MCP servers with namespacing
    pub fn list_prompts(&self) -> impl Future<Output = Result<Vec<rmcp::model::Prompt>>> + Send + 'static {
        let clients: Vec<_> = self
            .servers
            .iter()
            .filter_map(|(server_name, record)| {
                let conn = record.connection()?;
                conn.client.peer_info()?.capabilities.prompts.as_ref()?;
                Some((server_name.clone(), conn.client.clone()))
            })
            .collect();

        async move {
            let futures = clients.into_iter().map(|(server_name, client)| async move {
                let prompts_response = client.list_prompts(None).await.map_err(|e| {
                    McpError::PromptListFailed(format!("Failed to list prompts for {server_name}: {e}"))
                })?;

                Ok::<_, McpError>(
                    prompts_response
                        .prompts
                        .into_iter()
                        .map(|prompt| {
                            let namespaced_name = create_namespaced_tool_name(&server_name, &prompt.name);
                            rmcp::model::Prompt::new(namespaced_name, prompt.description, prompt.arguments)
                        })
                        .collect::<Vec<_>>(),
                )
            });

            let mut all_prompts = Vec::new();
            for result in join_all(futures).await {
                all_prompts.extend(result?);
            }
            Ok(all_prompts)
        }
    }

    /// Get a specific prompt by namespaced name
    pub fn get_prompt(
        &self,
        namespaced_prompt_name: &str,
        arguments: Option<serde_json::Map<String, serde_json::Value>>,
    ) -> impl Future<Output = Result<rmcp::model::GetPromptResult>> + Send + 'static {
        let resolved = split_on_server_name(namespaced_prompt_name)
            .ok_or_else(|| McpError::InvalidToolNameFormat(namespaced_prompt_name.to_string()))
            .and_then(|(server_name, prompt_name)| {
                let client = self
                    .connection_for(server_name)
                    .map(|connection| connection.client.clone())
                    .ok_or_else(|| McpError::ServerNotFound(server_name.to_string()))?;
                Ok((client, server_name.to_string(), prompt_name.to_string()))
            });

        async move {
            let (client, server_name, prompt_name) = resolved?;
            let mut request = rmcp::model::GetPromptRequestParams::new(prompt_name.clone());
            if let Some(args) = arguments {
                request = request.with_arguments(args);
            }

            client.get_prompt(request).await.map_err(|e| {
                McpError::PromptGetFailed(format!("Failed to get prompt '{prompt_name}' from {server_name}: {e}"))
            })
        }
    }

    /// Shutdown all servers and wait for their tasks to complete
    pub async fn shutdown(&mut self) {
        let servers: Vec<(String, ServerRecord)> = self.servers.drain().collect();

        for (server_name, record) in servers {
            if let Some(conn) = record.into_connection()
                && let Some(handle) = conn.server_task
            {
                drop(conn.client);
                await_server_shutdown(&server_name, handle).await;
            }
        }

        self.server_order.clear();
    }

    /// Shutdown a specific server by name
    pub async fn shutdown_server(&mut self, server_name: &str) -> Result<()> {
        if let Some(record) = self.servers.remove(server_name)
            && let Some(conn) = record.into_connection()
            && let Some(handle) = conn.server_task
        {
            drop(conn.client);
            await_server_shutdown(server_name, handle).await;
        }

        Ok(())
    }

    async fn emit_server_statuses_changed(&self) {
        self.emit_event(McpClientEvent::ServerStatusesChanged(self.server_statuses())).await;
    }

    async fn emit_tool_definitions_changed(&self) {
        self.emit_event(McpClientEvent::ToolDefinitionsChanged(self.tool_definitions())).await;
    }

    async fn emit_catalog_instructions_changed(&self, server_name: &str) {
        let Some(record) = self.servers.get(server_name) else { return };
        let instructions = self
            .has_model_visible_tools(server_name, record)
            .then(|| record.connection().and_then(|connection| connection.instructions.clone()))
            .flatten();
        self.emit_event(McpClientEvent::ServerInstructionsUpdated { server: server_name.to_string(), instructions })
            .await;
        self.emit_event(McpClientEvent::ServerInstructionsUpdated {
            server: PROGRESSIVE_DISCOVERY_INSTRUCTION_NAME.to_string(),
            instructions: self.progressive_discovery_instructions(),
        })
        .await;
    }

    async fn emit_instructions_after_connect(&self, server_name: &str) {
        let Some(record) = self.servers.get(server_name) else { return };
        if self.has_model_visible_tools(server_name, record)
            && let Some(instructions) = record.connection().and_then(|conn| conn.instructions.as_ref()).cloned()
        {
            self.emit_event(McpClientEvent::ServerInstructionsUpdated {
                server: server_name.to_string(),
                instructions: Some(instructions),
            })
            .await;
        }

        if record.exposure.has_deferred_tools()
            && let Some(body) = self.progressive_discovery_instructions()
        {
            self.emit_event(McpClientEvent::ServerInstructionsUpdated {
                server: PROGRESSIVE_DISCOVERY_INSTRUCTION_NAME.to_string(),
                instructions: Some(body),
            })
            .await;
        }
    }

    pub async fn emit_connection_ready(&self) {
        self.emit_event(McpClientEvent::ConnectionReady(McpConnectionDetails {
            tool_definitions: self.tool_definitions(),
            instructions: self.server_instructions(),
            server_statuses: self.server_statuses(),
        }))
        .await;
    }

    async fn emit_authentication_failed(&self, server: String, error: String) {
        self.emit_event(McpClientEvent::AuthenticationFailed { server, error }).await;
    }

    async fn emit_event(&self, event: McpClientEvent) {
        if let Err(e) = self.event_sender.send(event).await {
            tracing::warn!("Failed to emit MCP client event: {e}");
        }
    }

    fn connect_config(&self) -> Arc<ConnectConfig> {
        Arc::new(ConnectConfig {
            client_info: self.client_info.clone(),
            event_sender: self.event_sender.clone(),
            manager_event_sender: self.manager_event_sender.clone(),
            root_dir: self.root_dir.clone(),
            oauth_handler_factory: self.oauth_handler_factory.clone(),
            oauth_credential_store: self.oauth_credential_store.clone(),
        })
    }

    fn register_connection(
        &mut self,
        name: &str,
        conn: McpServerConnection,
        tools: Vec<Tool>,
        reauth_config: Option<McpHttpConfig>,
    ) -> Result<()> {
        let record = self.servers.get_mut(name).ok_or_else(|| McpError::ServerNotFound(name.to_string()))?;
        record.reauth_config = reauth_config.or_else(|| record.reauth_config.clone());
        record.oauth_challenge = None;
        record.state = ServerState::Connected { connection: conn, tools };
        Ok(())
    }

    fn remember_server_order(&mut self, name: &str) {
        if !self.server_order.iter().any(|n| n == name) {
            self.server_order.push(name.to_string());
        }
    }

    async fn apply_authentication_failure(&mut self, name: String, error: String) {
        self.set_state(&name, ServerState::Failed { error: error.clone() });
        self.emit_server_statuses_changed().await;
        self.emit_authentication_failed(name, error).await;
    }

    fn set_state(&mut self, name: &str, state: ServerState) {
        self.remember_server_order(name);
        match self.servers.get_mut(name) {
            Some(record) => record.state = state,
            None => {
                self.servers.insert(name.to_string(), ServerRecord::new(state, None, ToolExposure::ModelVisible));
            }
        }
    }

    fn register_record(
        &mut self,
        name: &str,
        state: ServerState,
        reauth_config: Option<McpHttpConfig>,
        exposure: ToolExposure,
    ) {
        self.remember_server_order(name);
        self.servers.insert(name.to_string(), ServerRecord::new(state, reauth_config, exposure));
    }

    fn connection_for(&self, server_name: &str) -> Option<&McpServerConnection> {
        self.servers.get(server_name).and_then(ServerRecord::connection)
    }

    fn client_for_server(&self, server_name: &str) -> Option<Arc<RunningService<RoleClient, McpClient>>> {
        self.connection_for(server_name).map(|conn| conn.client.clone())
    }

    fn deferred_record(&self, server: &str) -> Result<(&ServerRecord, &McpServerConnection)> {
        let record = self.servers.get(server).ok_or_else(|| McpError::ServerNotFound(server.to_string()))?;
        if !record.exposure.has_deferred_tools() {
            return Err(McpError::ServerHasNoDeferredTools(server.to_string()));
        }
        let connection = record.connection().ok_or_else(|| McpError::DeferredServerNotConnected(server.to_string()))?;
        Ok((record, connection))
    }

    fn is_routable_tool(&self, namespaced_tool_name: &str) -> bool {
        match split_on_server_name(namespaced_tool_name) {
            Some((server_name, tool_name)) => {
                self.servers.get(server_name).is_some_and(|record| {
                    record.tools().iter().find(|tool| tool.name == tool_name).is_some_and(|tool| {
                        record.is_model_visible(tool_name) && self.is_tool_allowed(server_name, tool)
                    })
                })
            }
            None => false,
        }
    }

    fn has_model_visible_tools(&self, server: &str, record: &ServerRecord) -> bool {
        record.tools().iter().any(|tool| record.is_model_visible(&tool.name) && self.is_tool_allowed(server, tool))
    }

    fn is_tool_allowed(&self, server: &str, tool: &Tool) -> bool {
        self.tool_filter.is_tool_allowed(&namespaced_tool_definition(server, tool))
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

/// Internal record holding all mutable state for a single MCP server.
struct ServerRecord {
    state: ServerState,
    reauth_config: Option<McpHttpConfig>,
    oauth_challenge: Option<String>,
    exposure: ToolExposure,
}

enum ServerState {
    Connecting,
    Connected { connection: McpServerConnection, tools: Vec<Tool> },
    Authenticating,
    Failed { error: String },
    NeedsOAuth,
}

impl From<&ServerState> for McpServerStatus {
    fn from(state: &ServerState) -> Self {
        match state {
            ServerState::Connecting => Self::Connecting,
            ServerState::Connected { tools, .. } => Self::Connected { tool_count: tools.len() },
            ServerState::Authenticating => Self::Authenticating,
            ServerState::Failed { error } => Self::Failed { error: error.clone() },
            ServerState::NeedsOAuth => Self::NeedsOAuth,
        }
    }
}

impl ServerRecord {
    fn new(state: ServerState, reauth_config: Option<McpHttpConfig>, exposure: ToolExposure) -> Self {
        Self { state, reauth_config, oauth_challenge: None, exposure }
    }

    fn tools(&self) -> &[Tool] {
        match &self.state {
            ServerState::Connected { tools, .. } => tools,
            ServerState::Connecting
            | ServerState::Authenticating
            | ServerState::Failed { .. }
            | ServerState::NeedsOAuth => &[],
        }
    }

    fn description(&self, name: &str) -> String {
        self.connection()
            .and_then(|connection| connection.client.peer_info())
            .and_then(|info| {
                info.server_info
                    .as_ref()
                    .and_then(|server_info| server_info.description.as_deref())
                    .filter(|description| !description.is_empty())
                    .map(ToString::to_string)
            })
            .unwrap_or_else(|| name.to_string())
    }

    fn is_model_visible(&self, tool_name: &str) -> bool {
        self.exposure.is_model_visible(tool_name)
    }

    fn is_deferred(&self, tool_name: &str) -> bool {
        self.exposure.is_deferred(tool_name)
    }

    fn replace_tools(&mut self, next_tools: Vec<Tool>) -> bool {
        match &mut self.state {
            ServerState::Connected { tools, .. } => {
                *tools = next_tools;
                true
            }
            _ => false,
        }
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

    fn can_authenticate(&self) -> bool {
        self.reauth_config.is_some()
    }

    fn status(&self) -> McpServerStatus {
        (&self.state).into()
    }

    fn status_entry(&self, name: &str) -> McpServerStatusEntry {
        McpServerStatusEntry::new(name, self.status())
            .with_auth_capability(self.auth_capability())
            .with_deferred_tools(self.exposure.has_deferred_tools())
    }
}

fn namespaced_tool_definition(server: &str, tool: &Tool) -> ToolDefinition {
    ToolDefinition::new(
        create_namespaced_tool_name(server, &tool.name),
        tool.description.clone(),
        tool.parameters.clone(),
    )
    .with_server(server)
    .with_annotations(tool.annotations.clone())
}

fn deferred_tool_definition(server: &str, tool: &Tool) -> ToolDefinition {
    ToolDefinition::new(tool.name.clone(), tool.description.clone(), tool.parameters.clone())
        .with_server(server)
        .with_annotations(tool.annotations.clone())
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
        McpClientEvent, McpManager, McpServerStatus, PROGRESSIVE_DISCOVERY_INSTRUCTION_NAME, ServerState, ToolFilter,
    };
    use crate::client::config::{McpHttpConfig, McpServer, McpTransport, ToolExposure};
    use crate::client::connection::{McpConnectAttempt, McpConnectOutcome};
    use crate::client::{OAuthHandlerFactory, ToolMatcher};
    use crate::status::McpServerAuthCapability;
    use aether_auth::{OAuthError, OAuthHandler};
    use futures::future::BoxFuture;
    use rmcp::{
        Json, RoleServer, ServerHandler,
        handler::server::{
            router::tool::{ToolRoute, ToolRouter},
            tool::ToolCallContext,
            wrapper::Parameters,
        },
        model::{CallToolResponse, CallToolResult, Implementation, ListToolsResult, ServerCapabilities, ServerInfo},
        service::{DynService, MaybeSendFuture, NotificationContext},
        tool, tool_handler, tool_router,
        transport::streamable_http_client::StreamableHttpClientTransportConfig,
    };
    use schemars::JsonSchema;
    use serde::{Deserialize, Serialize};
    use std::{
        io,
        sync::{Arc, Mutex},
    };
    use tokio::sync::{Notify, RwLock, mpsc};

    #[derive(Clone)]
    struct TestServer {
        tool_router: ToolRouter<Self>,
    }

    #[tool_handler(router = self.tool_router)]
    impl ServerHandler for TestServer {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
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
    struct ChangingToolServer {
        router: Arc<RwLock<ToolRouter<Self>>>,
        remove_tool: Arc<Notify>,
    }

    impl ChangingToolServer {
        fn new() -> Self {
            let mut router = ToolRouter::new();
            router.add_route(ToolRoute::new_dyn(
                rmcp::model::Tool::new("changing", "A changing tool", Arc::new(serde_json::Map::default())),
                |_| Box::pin(async { Ok(CallToolResult::default().into()) }),
            ));
            Self { router: Arc::new(RwLock::new(router)), remove_tool: Arc::new(Notify::new()) }
        }
    }

    impl ServerHandler for ChangingToolServer {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
        }

        async fn call_tool(
            &self,
            request: rmcp::model::CallToolRequestParams,
            context: rmcp::service::RequestContext<RoleServer>,
        ) -> std::result::Result<CallToolResponse, rmcp::ErrorData> {
            self.router.read().await.call(ToolCallContext::new(self, request, context)).await
        }

        async fn list_tools(
            &self,
            _request: Option<rmcp::model::PaginatedRequestParams>,
            _context: rmcp::service::RequestContext<RoleServer>,
        ) -> std::result::Result<ListToolsResult, rmcp::ErrorData> {
            Ok(ListToolsResult::with_all_items(self.router.read().await.list_all()))
        }

        fn on_initialized(
            &self,
            context: NotificationContext<RoleServer>,
        ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
            let router = self.router.clone();
            let remove_tool = self.remove_tool.clone();
            let peer = context.peer.clone();
            async move {
                router.write().await.bind_peer_notifier(&peer);
                tokio::spawn(async move {
                    remove_tool.notified().await;
                    router.write().await.disable_route("changing");
                });
            }
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
        let mut manager = McpManager::new(event_sender, Some(test_oauth_handler_factory()));
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
        let mut manager = McpManager::new(event_sender, Some(test_oauth_handler_factory()));
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
        let mut manager = McpManager::new(event_sender, Some(test_oauth_handler_factory()));
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
        let mut manager = McpManager::new(event_sender, Some(test_oauth_handler_factory()));

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

    #[tokio::test]
    async fn register_pending_marks_every_server_connecting_and_emits_status() {
        let (event_sender, mut event_receiver) = mpsc::channel(32);
        let mut manager = McpManager::new(event_sender, None);

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
    async fn apply_connection_attempt_emits_instructions_updated_after_connect() {
        let (event_sender, mut event_receiver) = mpsc::channel(32);
        let mut manager = McpManager::new(event_sender, None);

        let servers = vec![McpServer::new(
            "test",
            McpTransport::InMemory { server: TestServer::default().into_dyn() },
            ToolExposure::ModelVisible,
        )];
        manager.add_mcps(servers).await.unwrap();

        let mut update_for_test = None;
        while let Ok(event) = event_receiver.try_recv() {
            if let McpClientEvent::ServerInstructionsUpdated { server, instructions } = event
                && server == "test"
            {
                update_for_test = Some(instructions);
            }
        }
        let instructions = update_for_test.expect("expected ServerInstructionsUpdated for 'test'");
        assert!(instructions.is_some(), "TestServer publishes instructions, so update should carry Some(_)");
    }

    #[tokio::test]
    async fn server_statuses_mark_model_visible_and_deferred_servers_without_synthetic_row() {
        let (event_sender, _event_receiver) = mpsc::channel(32);
        let mut manager = McpManager::new(event_sender, None);
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
        assert!(!statuses.iter().any(|status| status.name == PROGRESSIVE_DISCOVERY_INSTRUCTION_NAME));

        assert_eq!(
            manager.tool_definitions().iter().map(|tool| tool.name.as_str()).collect::<Vec<_>>(),
            ["direct__echo"]
        );
        let servers = manager.deferred_servers();
        assert_eq!(servers.iter().map(|server| server.name.as_str()).collect::<Vec<_>>(), ["math"]);
        assert_eq!(manager.list_deferred_tools("math").await.unwrap()[0].name, "echo");
        let definition = manager.deferred_tool("math", "echo").await.unwrap();
        assert_eq!(definition.server.as_deref(), Some("math"));
        assert_eq!(definition.name, "echo");
        assert!(manager.resolve_deferred_tool("math", "echo", serde_json::Map::new()).is_ok());
        assert!(manager.resolve_deferred_tool("direct", "echo", serde_json::Map::new()).is_err());
    }

    #[tokio::test]
    async fn tool_filter_controls_discovery_and_execution() {
        let (event_sender, _event_receiver) = mpsc::channel(32);
        let filter = ToolFilter { allow: vec![ToolMatcher::name("direct__echo")], deny: Vec::new() };
        let mut manager = McpManager::new(event_sender, None).with_tool_filter(filter);
        manager
            .add_mcps(vec![
                McpServer::new(
                    "direct",
                    McpTransport::InMemory { server: TestServer::default().into_dyn() },
                    ToolExposure::ModelVisible,
                ),
                McpServer::new(
                    "blocked",
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

        assert_eq!(
            manager.tool_definitions().iter().map(|tool| tool.name.as_str()).collect::<Vec<_>>(),
            ["direct__echo"]
        );
        assert!(manager.get_client_for_tool("direct__echo", "{}").is_ok());
        assert!(manager.get_client_for_tool("blocked__echo", "{}").is_err());
        assert!(manager.deferred_servers().is_empty());
        assert!(manager.list_deferred_tools("math").await.unwrap().is_empty());
        assert!(manager.deferred_tool("math", "echo").await.is_err());
        assert!(manager.resolve_deferred_tool("math", "echo", serde_json::Map::new()).is_err());
        assert!(manager.progressive_discovery_instructions().is_none());
    }

    #[tokio::test]
    async fn annotation_filter_blocks_discovery_and_execution() {
        let (event_sender, _event_receiver) = mpsc::channel(32);
        let filter = ToolFilter { allow: Vec::new(), deny: vec![ToolMatcher::read_only()] };
        let mut manager = McpManager::new(event_sender, None).with_tool_filter(filter);
        manager
            .add_mcps(vec![
                McpServer::new(
                    "direct",
                    McpTransport::InMemory { server: TestServer::default().into_dyn() },
                    ToolExposure::ModelVisible,
                ),
                McpServer::new(
                    "deferred",
                    McpTransport::InMemory { server: TestServer::default().into_dyn() },
                    ToolExposure::deferred_all(),
                ),
            ])
            .await
            .unwrap();

        assert!(manager.tool_definitions().is_empty());
        assert!(manager.get_client_for_tool("direct__echo", "{}").is_err());
        assert!(manager.deferred_servers().is_empty());
        assert!(manager.list_deferred_tools("deferred").await.unwrap().is_empty());
        assert!(manager.resolve_deferred_tool("deferred", "echo", serde_json::Map::new()).is_err());
    }

    #[tokio::test]
    async fn tool_list_changed_notification_updates_catalog() {
        let (event_sender, _event_receiver) = mpsc::channel(32);
        let mut manager = McpManager::new(event_sender, None);
        let mut manager_events = manager.take_event_receiver();
        let server = ChangingToolServer::new();
        let remove_tool = server.remove_tool.clone();
        manager
            .add_mcps(vec![McpServer::new(
                "changing",
                McpTransport::InMemory { server: Box::new(server) },
                ToolExposure::ModelVisible,
            )])
            .await
            .unwrap();
        assert_eq!(manager.tool_definitions()[0].name, "changing__changing");

        remove_tool.notify_one();
        manager.apply_event(manager_events.recv().await.unwrap()).await;

        assert!(manager.tool_definitions().is_empty());
        assert!(manager.get_client_for_tool("changing__changing", "{}").is_err());
    }

    #[tokio::test]
    async fn tool_definitions_drop_when_a_server_shuts_down() {
        let (event_sender, _event_receiver) = mpsc::channel(32);
        let mut manager = McpManager::new(event_sender, None);
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

        let names =
            |manager: &McpManager| manager.tool_definitions().into_iter().map(|tool| tool.name).collect::<Vec<_>>();
        assert!(names(&manager).contains(&"git__echo".to_string()));
        assert!(names(&manager).contains(&"github__echo".to_string()));

        manager.shutdown_server("git").await.unwrap();

        assert!(!names(&manager).iter().any(|name| name.starts_with("git__")));
        assert!(names(&manager).contains(&"github__echo".to_string()));
    }

    #[tokio::test]
    async fn tool_definitions_preserve_annotations() {
        let (event_sender, _event_receiver) = mpsc::channel(32);
        let mut manager = McpManager::new(event_sender, None);
        manager
            .add_mcps(vec![McpServer::new(
                "test",
                McpTransport::InMemory { server: TestServer::default().into_dyn() },
                ToolExposure::ModelVisible,
            )])
            .await
            .unwrap();

        let tools = manager.tool_definitions();
        let echo = tools.iter().find(|tool| tool.name == "test__echo").expect("echo tool");
        let annotations = echo.annotations.as_ref().expect("annotations should be preserved");
        assert_eq!(annotations.read_only_hint, Some(true));
        assert_eq!(annotations.open_world_hint, Some(false));
    }

    #[tokio::test]
    async fn drop_logs_cleanup_abort_with_tracing() {
        let (event_sender, _event_receiver) = mpsc::channel(32);
        let mut manager = McpManager::new(event_sender, None);
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
}
