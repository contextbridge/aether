use llm::ToolDefinition;

use super::{
    McpError, Result,
    config::{McpHttpConfig, McpServer},
    connection::{
        ConnectConfig, McpConnectAttempt, McpConnectOutcome, McpServerConnection, Tool, authenticate_http,
        connect_server,
    },
    mcp_client::{McpClient, elicitation_capabilities},
    naming::{create_namespaced_tool_name, split_on_server_name},
    tool_proxy::ToolProxy,
};
use aether_auth::{OAuthCredentialStorage, OAuthHandler};
use futures::future::join_all;
use rmcp::{
    RoleClient,
    model::{
        CallToolRequestParams, ClientInfo, ElicitRequestParams, ElicitResult, ElicitationAction, Implementation,
        Tool as RmcpTool,
    },
    service::RunningService,
};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::num::NonZeroU16;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

pub use crate::status::{McpServerAuthCapability, McpServerStatus, McpServerStatusEntry};

pub const DEFAULT_PROXY_NAME: &str = "proxy";

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

/// Manages connections to multiple MCP servers and their tools
pub struct McpManager {
    servers: HashMap<String, ServerRecord>,
    server_order: Vec<String>,
    proxy: Option<ToolProxy>,
    aether_home: Option<PathBuf>,
    client_info: ClientInfo,
    event_sender: mpsc::Sender<McpClientEvent>,
    root_dir: PathBuf,
    oauth_handler_factory: Option<OAuthHandlerFactory>,
    oauth_credential_store: Option<Arc<dyn OAuthCredentialStorage>>,
}

impl McpManager {
    pub fn new(event_sender: mpsc::Sender<McpClientEvent>, oauth_handler_factory: Option<OAuthHandlerFactory>) -> Self {
        Self {
            servers: HashMap::new(),
            server_order: Vec::new(),
            proxy: None,
            aether_home: None,
            client_info: ClientInfo::new(elicitation_capabilities(), Implementation::new("aether", "0.1.0")),
            event_sender,
            root_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            oauth_handler_factory,
            oauth_credential_store: None,
        }
    }

    pub fn with_aether_home(mut self, aether_home: impl Into<PathBuf>) -> Self {
        self.aether_home = Some(aether_home.into());
        self
    }

    pub fn with_oauth_credential_store(mut self, store: Arc<dyn OAuthCredentialStorage>) -> Self {
        self.oauth_credential_store = Some(store);
        self
    }

    pub fn with_root_dir(mut self, root_dir: impl Into<PathBuf>) -> Self {
        self.root_dir = root_dir.into();
        self
    }

    pub async fn register_pending(&mut self, servers: Vec<McpServer>) -> Result<Vec<McpServer>> {
        let has_proxy = servers.iter().any(|server| server.proxy);
        if has_proxy && servers.iter().any(|server| server.name == DEFAULT_PROXY_NAME) {
            return Err(McpError::Other("server name 'proxy' collides with the tool proxy".into()));
        }

        for server in &servers {
            self.register_record(&server.name, ServerState::Connecting, None, server.proxy);
        }

        self.emit_server_statuses_changed().await;
        Ok(servers)
    }

    pub async fn bootstrap_proxy_setup(&mut self, servers: &[McpServer]) -> Result<()> {
        let proxied_members: HashSet<String> =
            servers.iter().filter(|server| server.proxy).map(|server| server.name.clone()).collect();

        if proxied_members.is_empty() {
            return Ok(());
        }

        let dir = self.proxy_tool_dir()?;
        ToolProxy::clean_dir(&dir).await?;
        self.register_proxy(dir, proxied_members);
        Ok(())
    }

    pub fn connect_pending_task(&self, server: McpServer) -> impl Future<Output = McpConnectAttempt> + Send + 'static {
        let ctx = self.connect_config();
        async move { connect_server(server, &ctx).await }
    }

    pub async fn add_mcps(&mut self, servers: Vec<McpServer>) -> Result<()> {
        self.bootstrap_proxy_setup(&servers).await?;
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

        if let Some(proxy) = self.proxy.as_ref().filter(|proxy| proxy.name() == server_name) {
            let call = proxy.resolve_call(arguments_json)?;
            let conn = self.connection_for(&call.server).ok_or_else(|| {
                McpError::ServerNotFound(format!("Proxied server '{}' is not connected", call.server))
            })?;
            let params = CallToolRequestParams::new(call.tool).with_arguments(call.arguments.unwrap_or_default());
            return Ok((conn.client.clone(), params));
        }

        let client =
            self.client_for_server(server_name).ok_or_else(|| McpError::ServerNotFound(server_name.to_string()))?;

        let arguments = serde_json::from_str::<serde_json::Value>(arguments_json)?.as_object().cloned();
        let mut params = CallToolRequestParams::new(tool_name.to_string());
        if let Some(args) = arguments {
            params = params.with_arguments(args);
        }

        Ok((client, params))
    }

    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        let mut definitions = Vec::new();
        if let Some(proxy) = self.proxy.as_ref() {
            definitions.push(ToolProxy::call_tool_definition(proxy.name()));
        }
        for name in &self.server_order {
            let Some(record) = self.servers.get(name) else { continue };
            if record.proxied {
                continue;
            }
            definitions.extend(record.tools().iter().map(|tool| {
                ToolDefinition::new(
                    create_namespaced_tool_name(name, &tool.name),
                    tool.description.clone(),
                    tool.parameters.clone(),
                )
                .with_server(name.clone())
                .with_annotations(tool.annotations.clone())
            }));
        }
        definitions
    }

    pub fn server_instructions(&self) -> BTreeMap<String, String> {
        let mut instructions: BTreeMap<String, String> = self
            .servers
            .iter()
            .filter(|(name, _)| self.proxy.as_ref().is_none_or(|proxy| !proxy.contains_server(name)))
            .filter_map(|(name, record)| {
                record
                    .connection()
                    .and_then(|conn| conn.instructions.as_ref())
                    .map(|instr| (name.clone(), instr.clone()))
            })
            .collect();

        if let Some((name, body)) = self.proxy_instructions() {
            instructions.insert(name, body);
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
        let proxied = record.proxied;
        let ctx = self.connect_config();

        self.set_state(&name, ServerState::Authenticating);
        self.emit_server_statuses_changed().await;

        Ok(async move { authenticate_http(name, config, ctx, proxied).await })
    }

    pub async fn apply_connection_attempt(&mut self, attempt: McpConnectAttempt) {
        let McpConnectAttempt { name, proxied, outcome } = attempt;
        match outcome {
            McpConnectOutcome::Connected { conn, reauth_config } => {
                match self.register_connection(&name, conn, reauth_config, proxied).await {
                    Ok(tools) => {
                        self.refresh_proxy_after_auth(&name, &tools, proxied).await;
                        self.emit_server_statuses_changed().await;
                        self.emit_tool_definitions_changed().await;
                        self.emit_instructions_after_connect(&name, proxied).await;
                    }
                    Err(error) => self.apply_authentication_failure(name, error.to_string()).await,
                }
            }
            McpConnectOutcome::NeedsOAuth { config, error } => {
                tracing::warn!("Server '{name}' needs OAuth: {error}");
                self.register_record(&name, ServerState::NeedsOAuth, Some(config), proxied);
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
                    let prompts_response = client.list_prompts(None).await.map_err(|e| {
                        McpError::PromptListFailed(format!("Failed to list prompts for {server_name}: {e}"))
                    })?;

                    let namespaced_prompts: Vec<rmcp::model::Prompt> = prompts_response
                        .prompts
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

        for (server_name, record) in servers {
            if let Some(conn) = record.into_connection()
                && let Some(handle) = conn.server_task
            {
                drop(conn.client);
                await_server_shutdown(&server_name, handle).await;
            }
        }

        self.server_order.clear();
        self.proxy = None;
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

    async fn emit_instructions_after_connect(&self, server_name: &str, proxied: bool) {
        if proxied {
            if let Some((server, body)) = self.proxy_instructions() {
                self.emit_event(McpClientEvent::ServerInstructionsUpdated { server, instructions: Some(body) }).await;
            }
            return;
        }

        if let Some(instructions) =
            self.connection_for(server_name).and_then(|conn| conn.instructions.as_ref()).cloned()
        {
            self.emit_event(McpClientEvent::ServerInstructionsUpdated {
                server: server_name.to_string(),
                instructions: Some(instructions),
            })
            .await;
        }
    }

    fn proxy_instructions(&self) -> Option<(String, String)> {
        let proxy = self.proxy.as_ref()?;
        let descriptions: Vec<(String, String)> = proxy
            .members()
            .iter()
            .filter_map(|member| {
                let conn = self.connection_for(member)?;
                Some((member.clone(), ToolProxy::extract_server_description(&conn.client, member)))
            })
            .collect();
        Some((proxy.name().to_string(), ToolProxy::build_instructions(proxy.tool_dir(), &descriptions)))
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
            root_dir: self.root_dir.clone(),
            oauth_handler_factory: self.oauth_handler_factory.clone(),
            oauth_credential_store: self.oauth_credential_store.clone(),
        })
    }

    fn proxy_tool_dir(&self) -> Result<PathBuf> {
        self.aether_home
            .as_ref()
            .map(|home| ToolProxy::dir_in_home(home, DEFAULT_PROXY_NAME))
            .map_or_else(|| ToolProxy::dir(DEFAULT_PROXY_NAME), Ok)
    }

    async fn register_connection(
        &mut self,
        name: &str,
        conn: McpServerConnection,
        reauth_config: Option<McpHttpConfig>,
        proxied: bool,
    ) -> Result<Vec<RmcpTool>> {
        let tools = conn
            .list_tools()
            .await
            .map_err(|e| McpError::ToolDiscoveryFailed(format!("Failed to list tools for {name}: {e}")))?;
        self.apply_connected(name, conn, &tools, reauth_config, proxied);
        Ok(tools)
    }

    fn apply_connected(
        &mut self,
        name: &str,
        conn: McpServerConnection,
        tools: &[RmcpTool],
        reauth_config: Option<McpHttpConfig>,
        proxied: bool,
    ) {
        let existing_reauth = self.servers.get(name).and_then(|r| r.reauth_config.clone());
        let final_reauth = reauth_config.or(existing_reauth);
        let tools = tools.iter().map(Tool::from).collect();

        self.remember_server_order(name);
        self.servers.insert(name.to_string(), ServerRecord::connected(conn, tools, final_reauth, proxied));
    }

    fn register_proxy(&mut self, tool_dir: std::path::PathBuf, members: HashSet<String>) {
        self.proxy = Some(ToolProxy::new(DEFAULT_PROXY_NAME.to_string(), members, tool_dir));
    }

    async fn refresh_proxy_after_auth(&mut self, name: &str, tools: &[RmcpTool], proxied: bool) {
        if !proxied {
            return;
        }

        if let Some(proxy) = self.proxy.as_mut() {
            proxy.add_member(name.to_string());
        }

        if let Some(tool_dir) = self.proxy.as_ref().map(|proxy| proxy.tool_dir().to_path_buf())
            && let Err(e) = ToolProxy::write_tool_entries_to_dir(name, tools, &tool_dir).await
        {
            tracing::warn!("Failed to write tool files for '{name}' after OAuth: {e}");
        }
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
                self.servers.insert(name.to_string(), ServerRecord::new(state, None, false));
            }
        }
    }

    fn register_record(&mut self, name: &str, state: ServerState, reauth_config: Option<McpHttpConfig>, proxied: bool) {
        self.remember_server_order(name);
        self.servers.insert(name.to_string(), ServerRecord::new(state, reauth_config, proxied));
    }

    fn connection_for(&self, server_name: &str) -> Option<&McpServerConnection> {
        self.servers.get(server_name).and_then(ServerRecord::connection)
    }

    fn client_for_server(&self, server_name: &str) -> Option<Arc<RunningService<RoleClient, McpClient>>> {
        self.connection_for(server_name).map(|conn| conn.client.clone())
    }

    fn is_routable_tool(&self, namespaced_tool_name: &str) -> bool {
        if self.proxy.as_ref().is_some_and(|proxy| proxy.call_tool_name() == namespaced_tool_name) {
            return true;
        }
        match split_on_server_name(namespaced_tool_name) {
            Some((server_name, tool_name)) => {
                self.servers.get(server_name).is_some_and(|record| !record.proxied && record.has_tool(tool_name))
            }
            None => false,
        }
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
    proxied: bool,
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
    fn new(state: ServerState, reauth_config: Option<McpHttpConfig>, proxied: bool) -> Self {
        Self { state, reauth_config, proxied }
    }

    fn connected(
        connection: McpServerConnection,
        tools: Vec<Tool>,
        reauth_config: Option<McpHttpConfig>,
        proxied: bool,
    ) -> Self {
        Self::new(ServerState::Connected { connection, tools }, reauth_config, proxied)
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

    fn has_tool(&self, tool_name: &str) -> bool {
        self.tools().iter().any(|tool| tool.name == tool_name)
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
            .with_proxied(self.proxied)
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
    use super::{DEFAULT_PROXY_NAME, McpClientEvent, McpManager, McpServerStatus, ServerState};
    use crate::client::OAuthHandlerFactory;
    use crate::client::config::{McpHttpConfig, McpServer, McpTransport};
    use crate::client::connection::{McpConnectAttempt, McpConnectOutcome};
    use crate::status::McpServerAuthCapability;
    use aether_auth::{OAuthCallback, OAuthError, OAuthHandler};
    use futures::future::BoxFuture;
    use rmcp::{
        Json, RoleServer, ServerHandler,
        handler::server::{router::tool::ToolRouter, wrapper::Parameters},
        model::{Implementation, ServerCapabilities, ServerInfo},
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

        fn authorize(&self, _auth_url: &str) -> BoxFuture<'_, Result<OAuthCallback, OAuthError>> {
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
        manager.register_record("public", ServerState::Connecting, None, false);

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
            false,
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
            false,
        );
        let _task = manager.authenticate_server_task("remote").await.expect("auth should start");
        let _authenticating_event = event_receiver.recv().await.expect("authenticating status change event");

        manager
            .apply_connection_attempt(McpConnectAttempt {
                name: "remote".to_string(),
                proxied: false,
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
            false,
        );
        manager.register_record("without-oauth", ServerState::Connecting, None, false);
        manager.register_record(
            "needs-oauth",
            ServerState::NeedsOAuth,
            Some(http_config("http://localhost/mcp2")),
            false,
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
            McpServer::new("alpha", McpTransport::InMemory { server: TestServer::default().into_dyn() }, false),
            McpServer::new("beta", McpTransport::InMemory { server: TestServer::default().into_dyn() }, true),
        ];

        let returned = manager.register_pending(servers).await.unwrap();
        assert_eq!(returned.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), vec!["alpha", "beta"]);

        let statuses = manager.server_statuses();
        assert_eq!(statuses.len(), 2);
        assert!(matches!(statuses.iter().find(|s| s.name == "alpha").unwrap().status, McpServerStatus::Connecting));
        assert!(matches!(statuses.iter().find(|s| s.name == "beta").unwrap().status, McpServerStatus::Connecting));
        assert!(statuses.iter().find(|s| s.name == "beta").unwrap().proxied);

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

        let servers =
            vec![McpServer::new("test", McpTransport::InMemory { server: TestServer::default().into_dyn() }, false)];
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
    async fn server_statuses_mark_direct_and_proxied_servers_without_proxy_row() {
        let (event_sender, _event_receiver) = mpsc::channel(32);
        let mut manager = McpManager::new(event_sender, None);
        manager
            .add_mcps(vec![
                McpServer::new("direct", McpTransport::InMemory { server: TestServer::default().into_dyn() }, false),
                McpServer::new("math", McpTransport::InMemory { server: TestServer::default().into_dyn() }, true),
            ])
            .await
            .unwrap();

        let statuses = manager.server_statuses();
        assert_eq!(statuses.iter().map(|status| status.name.as_str()).collect::<Vec<_>>(), vec!["direct", "math"]);
        assert!(!statuses.iter().find(|status| status.name == "direct").unwrap().proxied);
        assert!(statuses.iter().find(|status| status.name == "math").unwrap().proxied);
        assert!(!statuses.iter().any(|status| status.name == DEFAULT_PROXY_NAME));
    }

    #[tokio::test]
    async fn tool_definitions_drop_when_a_server_shuts_down() {
        let (event_sender, _event_receiver) = mpsc::channel(32);
        let mut manager = McpManager::new(event_sender, None);
        manager
            .add_mcps(vec![
                McpServer::new("git", McpTransport::InMemory { server: TestServer::default().into_dyn() }, false),
                McpServer::new("github", McpTransport::InMemory { server: TestServer::default().into_dyn() }, false),
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
                false,
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
                false,
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
