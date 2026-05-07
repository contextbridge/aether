use llm::ToolDefinition;

use super::{
    McpError, Result,
    config::McpServer,
    connection::{
        ConnectContext, ConnectedServer, ConnectionError, McpServerConnection, ServerInstructions, Tool,
        authenticate_http, connect_server,
    },
    mcp_client::McpClient,
    naming::{create_namespaced_tool_name, split_on_server_name},
    oauth::OAuthHandler,
    tool_proxy::ToolProxy,
};
use futures::future::join_all;
use rmcp::{
    RoleClient,
    model::{
        CallToolRequestParams, ClientCapabilities, ClientInfo, CreateElicitationRequestParams, CreateElicitationResult,
        ElicitationAction, FormElicitationCapability, Implementation, Root, Tool as RmcpTool, UrlElicitationCapability,
    },
    service::RunningService,
    transport::streamable_http_client::StreamableHttpClientTransportConfig,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc, oneshot};

pub use crate::status::{McpServerAuthCapability, McpServerStatus, McpServerStatusEntry};

pub const DEFAULT_PROXY_NAME: &str = "proxy";

pub type OAuthHandlerFactory = Arc<dyn Fn() -> Result<Arc<dyn OAuthHandler>> + Send + Sync>;

#[derive(Debug)]
pub struct ElicitationRequest {
    pub server_name: String,
    pub request: CreateElicitationRequestParams,
    pub response_sender: oneshot::Sender<CreateElicitationResult>,
}

#[derive(Debug, Clone)]
pub struct ElicitationResponse {
    pub action: ElicitationAction,
    pub content: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UrlElicitationCompleteParams {
    pub server_name: String,
    pub elicitation_id: String,
}

/// Events emitted by MCP clients that require attention from the host
/// (e.g. the relay or TUI). Flows through a single channel from `McpManager`
/// to the consumer.
#[derive(Debug)]
pub enum McpClientEvent {
    Elicitation(ElicitationRequest),
    UrlElicitationComplete(UrlElicitationCompleteParams),
    ServerStatusesChanged(Vec<McpServerStatusEntry>),
    ToolDefinitionsChanged(Vec<ToolDefinition>),
    AuthenticationFailed { server: String, error: String },
}

/// Manages connections to multiple MCP servers and their tools
pub struct McpManager {
    servers: HashMap<String, ServerRecord>,
    server_order: Vec<String>,
    tools: HashMap<String, Tool>,
    tool_definitions: Vec<ToolDefinition>,
    proxy: Option<ToolProxy>,
    aether_home: Option<PathBuf>,
    client_info: ClientInfo,
    event_sender: mpsc::Sender<McpClientEvent>,
    /// Roots shared with all MCP clients
    roots: Arc<RwLock<Vec<Root>>>,
    oauth_handler_factory: Option<OAuthHandlerFactory>,
    server_statuses: Vec<McpServerStatusEntry>,
}

impl McpManager {
    pub fn new(event_sender: mpsc::Sender<McpClientEvent>, oauth_handler_factory: Option<OAuthHandlerFactory>) -> Self {
        let mut capabilities = ClientCapabilities::builder().enable_elicitation().enable_roots().build();
        if let Some(elicitation) = capabilities.elicitation.as_mut() {
            elicitation.form = Some(FormElicitationCapability::default());
            elicitation.url = Some(UrlElicitationCapability::default());
        }

        Self {
            servers: HashMap::new(),
            server_order: Vec::new(),
            tools: HashMap::new(),
            tool_definitions: Vec::new(),
            proxy: None,
            aether_home: None,
            client_info: ClientInfo::new(capabilities, Implementation::new("aether", "0.1.0")),
            event_sender,
            roots: Arc::new(RwLock::new(Vec::new())),
            oauth_handler_factory,
            server_statuses: Vec::new(),
        }
    }

    pub fn with_aether_home(mut self, aether_home: impl Into<PathBuf>) -> Self {
        self.aether_home = Some(aether_home.into());
        self
    }

    pub async fn add_mcps(&mut self, servers: Vec<McpServer>) -> Result<()> {
        let has_proxy = servers.iter().any(|server| server.proxy);
        if has_proxy && servers.iter().any(|server| server.name == DEFAULT_PROXY_NAME) {
            return Err(McpError::Other("server name 'proxy' collides with the tool proxy".into()));
        }

        let proxied_members: HashSet<String> =
            servers.iter().filter(|server| server.proxy).map(|server| server.name.clone()).collect();
        let proxy_tool_dir = if has_proxy {
            let dir = self.proxy_tool_dir()?;
            ToolProxy::clean_dir(&dir).await?;
            Some(dir)
        } else {
            None
        };

        let ctx = self.connect_context();
        let outcomes = join_all(servers.into_iter().map(|server| connect_server(server, &ctx))).await;

        let mut connected_proxied = Vec::new();
        for outcome in outcomes {
            match outcome {
                Ok(ConnectedServer { name, conn, reauth_config, proxy }) => {
                    self.register_connection(&name, conn, reauth_config, proxy).await?;
                    if proxy {
                        connected_proxied.push(name);
                    }
                }
                Err(ConnectionError::NeedsOAuth { name, config, error, proxy }) => {
                    tracing::warn!("Server '{name}' needs OAuth: {error}");
                    self.upsert_status(&name, McpServerStatus::NeedsOAuth, Some(config), Some(proxy));
                }
                Err(ConnectionError::Failed { name, error, proxy }) => {
                    tracing::warn!("Failed to connect to MCP server '{name}': {error}");
                    if !self.servers.contains_key(&name) {
                        self.upsert_status(
                            &name,
                            McpServerStatus::Failed { error: error.to_string() },
                            None,
                            Some(proxy),
                        );
                    }
                }
            }
        }

        if let Some(tool_dir) = proxy_tool_dir {
            self.write_proxy_tool_files(&connected_proxied, &tool_dir).await;
            self.register_proxy(tool_dir, proxied_members);
        }

        Ok(())
    }

    pub fn get_client_for_tool(
        &self,
        namespaced_tool_name: &str,
        arguments_json: &str,
    ) -> Result<(Arc<RunningService<RoleClient, McpClient>>, CallToolRequestParams)> {
        if !self.tools.contains_key(namespaced_tool_name) {
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
        self.tool_definitions.clone()
    }

    pub fn server_instructions(&self) -> Vec<ServerInstructions> {
        let mut instructions: Vec<ServerInstructions> = self
            .servers
            .iter()
            .filter(|(name, _)| self.proxy.as_ref().is_none_or(|proxy| !proxy.contains_server(name)))
            .filter_map(|(name, record)| {
                record
                    .connection
                    .as_ref()
                    .and_then(|conn| conn.instructions.as_ref())
                    .map(|instr| ServerInstructions { server_name: name.clone(), instructions: instr.clone() })
            })
            .collect();

        if let Some(proxy) = &self.proxy {
            let descriptions: Vec<(String, String)> = proxy
                .members()
                .iter()
                .filter_map(|member| {
                    let conn = self.connection_for(member)?;
                    Some((member.clone(), ToolProxy::extract_server_description(&conn.client, member)))
                })
                .collect();
            instructions.push(ServerInstructions {
                server_name: proxy.name().to_string(),
                instructions: ToolProxy::build_instructions(proxy.tool_dir(), &descriptions),
            });
        }

        instructions
    }

    pub fn server_statuses(&self) -> &[McpServerStatusEntry] {
        &self.server_statuses
    }

    pub async fn authenticate_server_task(
        &mut self,
        name: &str,
    ) -> Result<impl Future<Output = std::result::Result<ConnectedServer, ConnectionError>> + Send + 'static> {
        let record = self
            .servers
            .get(name)
            .ok_or_else(|| McpError::ConnectionFailed(format!("server '{name}' is not OAuth-authenticatable")))?;
        if !record.can_authenticate() {
            return Err(McpError::ConnectionFailed(format!("server '{name}' is not OAuth-authenticatable")));
        }
        if matches!(record.status, McpServerStatus::Authenticating) {
            return Err(McpError::ConnectionFailed(format!("server '{name}' is already authenticating")));
        }

        let oauth_handler_factory = self
            .oauth_handler_factory
            .clone()
            .ok_or_else(|| McpError::ConnectionFailed(format!("No OAuth handler factory available for '{name}'")))?;
        let name = name.to_string();
        let config = record.reauth_config.clone().expect("checked above");
        let client_info = self.client_info.clone();
        let event_sender = self.event_sender.clone();
        let roots = Arc::clone(&self.roots);
        let proxy = record.proxy;

        self.upsert_status(&name, McpServerStatus::Authenticating, Some(config.clone()), None);
        self.emit_server_statuses_changed().await;

        Ok(async move {
            authenticate_http(name, config, client_info, event_sender, roots, oauth_handler_factory, proxy).await
        })
    }

    pub async fn apply_connection_attempt(&mut self, attempt: std::result::Result<ConnectedServer, ConnectionError>) {
        match attempt {
            Ok(ConnectedServer { name, conn, reauth_config, proxy }) => {
                match self.register_connection(&name, conn, reauth_config, proxy).await {
                    Ok(tools) => {
                        self.refresh_proxy_after_auth(&name, &tools, proxy).await;
                        self.emit_server_statuses_changed().await;
                        self.emit_tool_definitions_changed().await;
                    }
                    Err(error) => {
                        self.apply_authentication_failure(name, error.to_string()).await;
                    }
                }
            }
            Err(ConnectionError::Failed { name, error, .. }) => {
                self.apply_authentication_failure(name, error.to_string()).await;
            }
            Err(ConnectionError::NeedsOAuth { name, .. }) => {
                self.apply_authentication_failure(name, "internal error: auth task returned NeedsOAuth".to_string())
                    .await;
            }
        }
    }

    /// List all prompts from all connected MCP servers with namespacing
    pub async fn list_prompts(&self) -> Result<Vec<rmcp::model::Prompt>> {
        let futures: Vec<_> = self
            .servers
            .iter()
            .filter_map(|(server_name, record)| {
                let conn = record.connection.as_ref()?;
                conn.client.peer_info().and_then(|info| info.capabilities.prompts.as_ref())?;
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
            if let Some(conn) = record.connection
                && let Some(handle) = conn.server_task
            {
                drop(conn.client);

                match tokio::time::timeout(std::time::Duration::from_secs(5), handle).await {
                    Ok(Ok(())) => {
                        tracing::info!("Server '{server_name}' shut down gracefully");
                    }
                    Ok(Err(e)) => {
                        tracing::warn!("Server '{server_name}' task panicked: {e:?}");
                    }
                    Err(_) => {
                        tracing::warn!("Server '{server_name}' shutdown timed out");
                    }
                }
            }
        }

        self.tools.clear();
        self.tool_definitions.clear();
        self.proxy = None;
    }

    /// Shutdown a specific server by name
    pub async fn shutdown_server(&mut self, server_name: &str) -> Result<()> {
        let record = self.servers.remove(server_name);

        if let Some(record) = record {
            if let Some(conn) = record.connection
                && let Some(handle) = conn.server_task
            {
                drop(conn.client);

                match tokio::time::timeout(std::time::Duration::from_secs(5), handle).await {
                    Ok(Ok(())) => {
                        tracing::info!("Server '{server_name}' shut down gracefully");
                    }
                    Ok(Err(e)) => {
                        tracing::warn!("Server '{server_name}' task panicked: {e:?}");
                    }
                    Err(_) => {
                        tracing::warn!("Server '{server_name}' shutdown timed out");
                    }
                }
            }

            self.remove_registered_tools_for_server(server_name);
            self.refresh_status_entries();
        }

        Ok(())
    }

    /// Set the roots advertised to MCP servers.
    ///
    /// This updates the roots and sends notifications to all connected servers
    /// that support the `roots/list_changed` notification.
    pub async fn set_roots(&mut self, new_roots: Vec<Root>) -> Result<()> {
        {
            let mut roots = self.roots.write().await;
            *roots = new_roots;
        }

        self.notify_roots_changed().await;

        Ok(())
    }

    async fn emit_server_statuses_changed(&self) {
        self.emit_event(McpClientEvent::ServerStatusesChanged(self.server_statuses().to_vec())).await;
    }

    async fn emit_tool_definitions_changed(&self) {
        self.emit_event(McpClientEvent::ToolDefinitionsChanged(self.tool_definitions())).await;
    }

    async fn emit_authentication_failed(&self, server: String, error: String) {
        self.emit_event(McpClientEvent::AuthenticationFailed { server, error }).await;
    }

    async fn emit_event(&self, event: McpClientEvent) {
        if let Err(e) = self.event_sender.send(event).await {
            tracing::warn!("Failed to emit MCP client event: {e}");
        }
    }

    fn connect_context(&self) -> ConnectContext<'_> {
        ConnectContext {
            client_info: &self.client_info,
            event_sender: &self.event_sender,
            roots: &self.roots,
            oauth_handler_factory: self.oauth_handler_factory.as_ref(),
        }
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
        reauth_config: Option<StreamableHttpClientTransportConfig>,
        proxy: bool,
    ) -> Result<Vec<RmcpTool>> {
        let tools = conn
            .list_tools()
            .await
            .map_err(|e| McpError::ToolDiscoveryFailed(format!("Failed to list tools for {name}: {e}")))?;
        self.apply_connected(name, conn, &tools, reauth_config, proxy);
        Ok(tools)
    }

    fn apply_connected(
        &mut self,
        name: &str,
        conn: McpServerConnection,
        tools: &[RmcpTool],
        reauth_config: Option<StreamableHttpClientTransportConfig>,
        proxy: bool,
    ) {
        self.remove_registered_tools_for_server(name);

        let existing_reauth = self.servers.get(name).and_then(|r| r.reauth_config.clone());
        let final_reauth = reauth_config.or(existing_reauth);

        for rmcp_tool in tools {
            let tool_name = rmcp_tool.name.to_string();
            let namespaced_tool_name = create_namespaced_tool_name(name, &tool_name);
            let tool = Tool::from(rmcp_tool);

            if !proxy {
                self.tool_definitions.push(ToolDefinition {
                    name: namespaced_tool_name.clone(),
                    description: tool.description.clone(),
                    parameters: tool.parameters.to_string(),
                    server: Some(name.to_string()),
                });
                self.tools.insert(namespaced_tool_name, tool);
            }
        }

        self.remember_server_order(name);
        self.servers.insert(name.to_string(), ServerRecord::connected(conn, tools.len(), final_reauth, proxy));
        self.refresh_status_entries();
    }

    fn register_proxy(&mut self, tool_dir: std::path::PathBuf, members: HashSet<String>) {
        self.remove_registered_tools_for_server(DEFAULT_PROXY_NAME);
        let call_tool_def = ToolProxy::call_tool_definition(DEFAULT_PROXY_NAME);
        self.tools.insert(
            call_tool_def.name.clone(),
            Tool {
                description: call_tool_def.description.clone(),
                parameters: serde_json::from_str(&call_tool_def.parameters)
                    .unwrap_or(Value::Object(serde_json::Map::default())),
            },
        );
        self.tool_definitions.push(call_tool_def);

        self.proxy = Some(ToolProxy::new(DEFAULT_PROXY_NAME.to_string(), members, tool_dir));
    }

    async fn refresh_proxy_after_auth(&mut self, name: &str, tools: &[RmcpTool], is_proxied: bool) {
        if !is_proxied {
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

    async fn write_proxy_tool_files(&self, connected_proxied: &[String], tool_dir: &std::path::Path) {
        let writes = connected_proxied.iter().filter_map(|name| {
            let client = self.client_for_server(name)?;
            let dir = tool_dir.to_path_buf();
            let name = name.clone();
            Some(async move {
                if let Err(e) = ToolProxy::write_tools_to_dir(&name, &client, &dir).await {
                    tracing::warn!("Failed to write tool files for proxied server '{name}': {e}");
                }
            })
        });
        join_all(writes).await;
    }

    fn refresh_status_entries(&mut self) {
        self.server_statuses = self
            .server_order
            .iter()
            .filter_map(|name| self.servers.get(name).map(|record| record.status_entry(name)))
            .collect();
    }

    fn remember_server_order(&mut self, name: &str) {
        if !self.server_order.iter().any(|n| n == name) {
            self.server_order.push(name.to_string());
        }
    }

    async fn apply_authentication_failure(&mut self, name: String, error: String) {
        self.mark_failed(&name, error.clone());
        self.emit_server_statuses_changed().await;
        self.emit_authentication_failed(name, error).await;
    }

    fn mark_failed(&mut self, name: &str, error: String) {
        let existing_reauth = self.servers.get(name).and_then(|record| record.reauth_config.clone());
        self.upsert_status(name, McpServerStatus::Failed { error }, existing_reauth, None);
    }

    fn upsert_status(
        &mut self,
        name: &str,
        status: McpServerStatus,
        reauth_config: Option<StreamableHttpClientTransportConfig>,
        proxy: Option<bool>,
    ) {
        self.remember_server_order(name);
        let record = self
            .servers
            .entry(name.to_string())
            .or_insert_with(|| ServerRecord::new(status.clone(), reauth_config.clone(), proxy.unwrap_or_default()));
        record.status = status;
        if reauth_config.is_some() {
            record.reauth_config = reauth_config;
        }
        if let Some(proxy) = proxy {
            record.proxy = proxy;
        }
        self.refresh_status_entries();
    }

    fn connection_for(&self, server_name: &str) -> Option<&McpServerConnection> {
        self.servers.get(server_name).and_then(|record| record.connection.as_ref())
    }

    fn client_for_server(&self, server_name: &str) -> Option<Arc<RunningService<RoleClient, McpClient>>> {
        self.connection_for(server_name).map(|conn| conn.client.clone())
    }

    fn remove_registered_tools_for_server(&mut self, server_name: &str) {
        let prefix = format!("{server_name}__");
        self.tools.retain(|tool_name, _| !tool_name.starts_with(&prefix));
        self.tool_definitions.retain(|tool_def| !tool_def.name.starts_with(&prefix));
    }

    async fn notify_roots_changed(&self) {
        for (server_name, record) in &self.servers {
            if let Some(conn) = &record.connection
                && let Err(e) = conn.client.notify_roots_list_changed().await
            {
                tracing::debug!("Note: server '{server_name}' did not accept roots notification: {e}");
            }
        }
    }
}

impl Drop for McpManager {
    fn drop(&mut self) {
        let servers: Vec<(String, ServerRecord)> = self.servers.drain().collect();
        for (server_name, record) in servers {
            if let Some(conn) = record.connection
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
    connection: Option<McpServerConnection>,
    status: McpServerStatus,
    reauth_config: Option<StreamableHttpClientTransportConfig>,
    proxy: bool,
}

impl ServerRecord {
    fn new(status: McpServerStatus, reauth_config: Option<StreamableHttpClientTransportConfig>, proxy: bool) -> Self {
        Self { connection: None, status, reauth_config, proxy }
    }

    fn connected(
        connection: McpServerConnection,
        tool_count: usize,
        reauth_config: Option<StreamableHttpClientTransportConfig>,
        proxy: bool,
    ) -> Self {
        Self { connection: Some(connection), status: McpServerStatus::Connected { tool_count }, reauth_config, proxy }
    }

    fn auth_capability(&self) -> McpServerAuthCapability {
        if self.reauth_config.is_some() { McpServerAuthCapability::OAuth } else { McpServerAuthCapability::Unavailable }
    }

    fn can_authenticate(&self) -> bool {
        self.reauth_config.is_some()
    }

    fn status_entry(&self, name: &str) -> McpServerStatusEntry {
        McpServerStatusEntry::new(name, self.status.clone())
            .with_auth_capability(self.auth_capability())
            .with_proxy(self.proxy)
    }
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_PROXY_NAME, McpClientEvent, McpManager, McpServerStatus, Tool};
    use crate::client::OAuthHandlerFactory;
    use crate::client::config::{McpServer, McpTransport};
    use crate::client::oauth::{OAuthCallback, OAuthError, OAuthHandler};
    use crate::status::McpServerAuthCapability;
    use futures::future::BoxFuture;
    use llm::ToolDefinition;
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
    use serde_json::json;
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

        #[tool(description = "Returns the provided value")]
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
        Arc::new(|| Ok(Arc::new(TestOAuthHandler)))
    }

    #[tokio::test]
    async fn authenticate_server_task_rejects_record_without_reauth_config() {
        let (event_sender, _event_receiver) = mpsc::channel(1);
        let mut manager = McpManager::new(event_sender, Some(test_oauth_handler_factory()));
        manager.upsert_status("public", McpServerStatus::Connected { tool_count: 1 }, None, None);

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
        manager.upsert_status(
            "remote",
            McpServerStatus::NeedsOAuth,
            Some(StreamableHttpClientTransportConfig::with_uri("http://localhost:19999/mcp")),
            None,
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
    async fn authenticate_server_task_rejects_duplicate_same_server_while_in_flight() {
        let (event_sender, _event_receiver) = mpsc::channel(1);
        let mut manager = McpManager::new(event_sender, Some(test_oauth_handler_factory()));
        manager.upsert_status(
            "remote",
            McpServerStatus::NeedsOAuth,
            Some(StreamableHttpClientTransportConfig::with_uri("http://localhost:19999/mcp")),
            None,
        );

        let _task = manager.authenticate_server_task("remote").await.expect("first auth should start");
        let error = match manager.authenticate_server_task("remote").await {
            Ok(_) => panic!("duplicate auth should be rejected"),
            Err(error) => error.to_string(),
        };

        assert!(error.contains("already authenticating"));
    }

    #[tokio::test]
    async fn apply_connection_attempt_failure_allows_retry() {
        let (event_sender, mut event_receiver) = mpsc::channel(2);
        let mut manager = McpManager::new(event_sender, Some(test_oauth_handler_factory()));
        manager.upsert_status(
            "remote",
            McpServerStatus::NeedsOAuth,
            Some(StreamableHttpClientTransportConfig::with_uri("http://localhost:19999/mcp")),
            None,
        );
        let _task = manager.authenticate_server_task("remote").await.expect("auth should start");
        let _authenticating_event = event_receiver.recv().await.expect("authenticating status change event");

        manager
            .apply_connection_attempt(Err(crate::client::ConnectionError::Failed {
                name: "remote".to_string(),
                error: crate::client::McpError::ConnectionFailed("boom".to_string()),
                proxy: false,
            }))
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

        manager.upsert_status(
            "with-oauth",
            McpServerStatus::Connected { tool_count: 1 },
            Some(StreamableHttpClientTransportConfig::with_uri("http://localhost/mcp")),
            None,
        );
        manager.upsert_status("without-oauth", McpServerStatus::Connected { tool_count: 2 }, None, None);
        manager.upsert_status(
            "needs-oauth",
            McpServerStatus::NeedsOAuth,
            Some(StreamableHttpClientTransportConfig::with_uri("http://localhost/mcp2")),
            None,
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
    async fn authentication_status_transitions_preserve_proxied_flag() {
        let (event_sender, _event_receiver) = mpsc::channel(2);
        let mut manager = McpManager::new(event_sender, Some(test_oauth_handler_factory()));
        manager.upsert_status(
            "remote",
            McpServerStatus::NeedsOAuth,
            Some(StreamableHttpClientTransportConfig::with_uri("http://localhost:19999/mcp")),
            Some(true),
        );

        let _task = manager.authenticate_server_task("remote").await.expect("auth should start");
        assert!(manager.server_statuses()[0].proxy);

        manager.mark_failed("remote", "boom".to_string());
        assert!(manager.server_statuses()[0].proxy);
    }

    #[tokio::test]
    async fn server_statuses_mark_direct_and_proxied_servers_without_proxy_row() {
        let (event_sender, _event_receiver) = mpsc::channel(1);
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
        assert!(!statuses.iter().find(|status| status.name == "direct").unwrap().proxy);
        assert!(statuses.iter().find(|status| status.name == "math").unwrap().proxy);
        assert!(!statuses.iter().any(|status| status.name == DEFAULT_PROXY_NAME));
    }

    #[test]
    fn remove_registered_tools_for_server_uses_namespaced_prefix() {
        let (event_sender, _event_receiver) = mpsc::channel(1);
        let mut manager = McpManager::new(event_sender, None);
        manager.tools.insert("git__status".to_string(), Tool { description: String::new(), parameters: json!({}) });
        manager.tools.insert("github__issue".to_string(), Tool { description: String::new(), parameters: json!({}) });
        manager.tool_definitions.push(ToolDefinition {
            name: "git__status".to_string(),
            description: String::new(),
            parameters: "{}".to_string(),
            server: Some("git".to_string()),
        });
        manager.tool_definitions.push(ToolDefinition {
            name: "github__issue".to_string(),
            description: String::new(),
            parameters: "{}".to_string(),
            server: Some("github".to_string()),
        });

        manager.remove_registered_tools_for_server("git");

        assert!(!manager.tools.contains_key("git__status"));
        assert!(manager.tools.contains_key("github__issue"));
        assert_eq!(
            manager.tool_definitions.iter().map(|tool| tool.name.as_str()).collect::<Vec<_>>(),
            vec!["github__issue"]
        );
    }

    #[tokio::test]
    async fn drop_logs_cleanup_abort_with_tracing() {
        let (event_sender, _event_receiver) = mpsc::channel(1);
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
