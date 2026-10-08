use super::LIST_SERVERS_TOOL;
use super::catalog::McpCatalog;
use super::servers::{Shared, with_servers};
use crate::McpError;
use crate::client::{ToolCall, ToolCallError, ToolCallEvent, ToolCallOptions};
use futures::StreamExt;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ErrorData, Implementation, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
};
use rmcp::{RoleServer, ServerHandler, service::RequestContext};
use serde_json::{Map, json};
use std::future::{Future, ready};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::watch;

pub(super) struct DeferredToolsMcp {
    servers: Shared,
    catalog: watch::Receiver<Arc<McpCatalog>>,
    call_timeout: Option<Duration>,
}

impl DeferredToolsMcp {
    pub(super) fn new(
        servers: Shared,
        catalog: watch::Receiver<Arc<McpCatalog>>,
        call_timeout: Option<Duration>,
    ) -> Self {
        Self { servers, catalog, call_timeout }
    }

    fn catalog(&self) -> Arc<McpCatalog> {
        Arc::clone(&self.catalog.borrow())
    }

    fn tools(&self) -> Vec<Tool> {
        let list_servers =
            Tool::new(LIST_SERVERS_TOOL, "List connected MCP servers with deferred tools", Arc::new(Map::new()));
        self.catalog().deferred_tools().cloned().chain([list_servers]).collect()
    }

    fn list_servers(&self) -> CallToolResult {
        let servers = self
            .catalog()
            .deferred_servers()
            .map(|(name, description)| json!({ "name": name, "description": description }))
            .collect::<Vec<_>>();
        CallToolResult::structured(servers.into())
    }
}

impl ServerHandler for DeferredToolsMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_server_info(Implementation::new(
            concat!(env!("CARGO_PKG_NAME"), "-deferred-tools"),
            env!("CARGO_PKG_VERSION"),
        ))
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

        let options = ToolCallOptions { timeout: self.call_timeout, meta: request.meta, cancel: context.ct.clone() };
        let args = request.arguments.unwrap_or_default();
        let call = with_servers(&self.servers, |servers| servers.call_deferred_tool(&request.name, args, options))
            .unwrap_or(Err(McpError::GatewayClosed))?;
        forward(call, &request.name, &context).await
    }
}

/// Passes an upstream `call` through to the downstream `tools/call` request named `name` that triggered it: relays
/// upstream progress under the request's progress token, logs the outcome, and returns the result as the response.
async fn forward(
    mut call: ToolCall,
    name: &str,
    context: &RequestContext<RoleServer>,
) -> Result<CallToolResponse, ErrorData> {
    let started = Instant::now();
    let progress_token = context.meta.get_progress_token();
    let mut result = Err(ToolCallError::Cancelled);
    while let Some(event) = call.next().await {
        match event {
            ToolCallEvent::Progress(mut progress) => {
                if let Some(token) = &progress_token {
                    progress.progress_token = token.clone();
                    let _ = context.peer.notify_progress(progress).await;
                }
            }
            ToolCallEvent::Done { result: outcome, .. } => {
                result = outcome;
                break;
            }
            ToolCallEvent::TaskCreated(_) | ToolCallEvent::TaskStatus(_) => {}
        }
    }

    let outcome = match &result {
        Ok(_) => "success",
        Err(ToolCallError::Cancelled) => "cancelled",
        Err(_) => "error",
    };
    tracing::info!(tool = name, outcome, duration_ms = started.elapsed().as_millis(), "MCP tool call forwarded");

    result.map(Into::into).map_err(|error| ErrorData::internal_error(error.to_string(), None))
}
