use super::mcp_catalog::{CatalogCore, Tier, namespaced};
use mcp_utils::client::{ClientError, ClientOptions, McpClient, ToolCall, ToolCallError, ToolCallOptions, Transport};
use mcp_utils::server::{McpServer, ServerHandle, UnixSocket};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ErrorData, Implementation, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
};
use rmcp::{RoleServer, ServerHandler, service::RequestContext};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::future::{Future, ready};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use thiserror::Error;

pub const AETHER_MCP_IPC_SOCKET: &str = "AETHER_MCP_IPC_SOCKET";
pub(crate) const PROGRESSIVE_DISCOVERY_INSTRUCTIONS: &str = include_str!("progressive_discovery_instructions.md");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeferredServer {
    pub name: String,
    pub description: String,
    pub tools: Vec<DeferredTool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeferredTool {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

pub struct DeferredToolsClient {
    client: McpClient,
}

#[derive(Debug, Error)]
pub enum DeferredToolsError {
    #[error("failed to connect to the active Aether session: {0}")]
    Connect(#[source] ClientError),
    #[error("deferred tool discovery timed out after {} seconds", .0.as_secs())]
    TimedOut(Duration),
    #[error("failed to list deferred servers: {0}")]
    ListServers(#[source] ToolCallError),
    #[error("invalid deferred server listing: {0}")]
    MalformedListing(#[source] serde_json::Error),
    #[error("failed to list deferred tools: {0}")]
    ListTools(#[source] ClientError),
}

impl DeferredToolsClient {
    pub async fn connect(socket: PathBuf) -> Result<Self, DeferredToolsError> {
        let options = ClientOptions::default().implementation(Implementation::new("aether", env!("CARGO_PKG_VERSION")));
        let client = McpClient::connect("aether", Transport::Unix(socket), &options).await;
        client.map(|client| Self { client }).map_err(DeferredToolsError::Connect)
    }

    pub async fn servers(&self) -> Result<Vec<DeferredServer>, DeferredToolsError> {
        let options = ToolCallOptions::with_timeout(DISCOVERY_TIMEOUT);
        let listed = self.client.call_tool(LIST_SERVERS_TOOL, Map::new(), options).result().await;
        let listed = listed.map_err(|error| match error {
            ToolCallError::TimedOut(timeout) => DeferredToolsError::TimedOut(timeout),
            error => DeferredToolsError::ListServers(error),
        })?;
        serde_json::from_value(listed.structured_content.unwrap_or_default())
            .map_err(DeferredToolsError::MalformedListing)
    }

    pub async fn tool(&self, server: &str, tool: &str) -> Result<Option<Tool>, DeferredToolsError> {
        let tools = tokio::time::timeout(DISCOVERY_TIMEOUT, self.client.list_tools())
            .await
            .map_err(|_| DeferredToolsError::TimedOut(DISCOVERY_TIMEOUT))?
            .map_err(DeferredToolsError::ListTools)?;
        let name = namespaced(server, tool);
        Ok(tools.into_iter().find(|definition| definition.name == name))
    }

    pub fn call(&self, server: &str, tool: &str, args: Map<String, Value>, timeout: Duration) -> ToolCall {
        self.client.call_tool(&namespaced(server, tool), args, ToolCallOptions::with_timeout(timeout))
    }

    pub async fn close(&self) {
        self.client.close().await;
    }
}

pub(super) fn serve(catalog: CatalogCore, socket: UnixSocket) -> ServerHandle {
    McpServer::new(DeferredToolsMcp { catalog }).serve_unix(socket)
}

const LIST_SERVERS_TOOL: &str = "_aether_list_servers";
const CALL_TIMEOUT: Duration = Duration::from_mins(10);
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(5);

struct DeferredToolsMcp {
    catalog: CatalogCore,
}

impl DeferredToolsMcp {
    fn tools(&self) -> Vec<Tool> {
        let list_servers =
            Tool::new(LIST_SERVERS_TOOL, "List connected MCP servers with deferred tools", Arc::new(Map::new()));
        let mut tools: Vec<Tool> = self.catalog.tools(Tier::Deferred).into_iter().map(|(_, tool)| tool).collect();
        tools.push(list_servers);
        tools
    }

    fn list_servers(&self) -> CallToolResult {
        let servers = self
            .catalog
            .servers(Tier::Deferred)
            .into_iter()
            .map(|(server, tools)| DeferredServer {
                name: server.name().to_string(),
                description: server.description().unwrap_or_else(|| server.name().to_string()),
                tools: tools
                    .into_iter()
                    .map(|tool| DeferredTool {
                        name: tool.name.to_string(),
                        description: tool.description.map(Into::into),
                    })
                    .collect(),
            })
            .collect::<Vec<_>>();
        CallToolResult::structured(serde_json::to_value(servers).expect("deferred servers serialize to JSON"))
    }
}

impl ServerHandler for DeferredToolsMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("aether-deferred-tools", env!("CARGO_PKG_VERSION")))
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, ErrorData>> + Send + '_ {
        ready(Ok(ListToolsResult::with_all_items(self.tools())))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if request.name == LIST_SERVERS_TOOL {
            return Ok(self.list_servers().into());
        }
        let name = &request.name;
        if self.catalog.tier_of(name) == Some(Tier::Direct) {
            let message = format!("Tool '{name}' is exposed directly; call '{name}' as a regular tool instead");
            return Err(ErrorData::invalid_params(message, None));
        }

        let options = ToolCallOptions { timeout: Some(CALL_TIMEOUT), meta: request.meta, cancel: context.ct.clone() };
        let args = request.arguments.unwrap_or_default();
        let Some(call) = self.catalog.call(Tier::Deferred, name, args, options) else {
            return Err(ErrorData::invalid_params(format!("Tool not found: {name}"), None));
        };
        forward(call, name, &context).await
    }
}

async fn forward(
    call: ToolCall,
    name: &str,
    context: &RequestContext<RoleServer>,
) -> Result<CallToolResponse, ErrorData> {
    let started = Instant::now();
    let progress_token = context.meta.get_progress_token();
    let result = call
        .result_with_progress(|mut progress| {
            let (peer, token) = (context.peer.clone(), progress_token.clone());
            async move {
                if let Some(token) = token {
                    progress.progress_token = token;
                    let _ = peer.notify_progress(progress).await;
                }
            }
        })
        .await;

    let outcome = match &result {
        Ok(_) => "success",
        Err(ToolCallError::Cancelled) => "cancelled",
        Err(_) => "error",
    };
    tracing::info!(tool = name, outcome, duration_ms = started.elapsed().as_millis(), "MCP tool call forwarded");

    result.map(Into::into).map_err(|error| ErrorData::internal_error(error.to_string(), None))
}
