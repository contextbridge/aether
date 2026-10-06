//! Serve every server behind an [`McpHandle`] as a single MCP server.
//!
//! [`AggregateServer`] is transport-agnostic: hand it to any rmcp transport, such as
//! stdio, a Unix socket, or a stateless streamable HTTP service.

use crate::client::{
    CallToolOptions, CancellationToken, CatalogTool, McpHandle, McpHandleError, ToolCallEvent, ToolRoute,
    split_on_server_name,
};
use futures::StreamExt;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ErrorData, GetPromptRequestParams, GetPromptResponse,
    Implementation, ListPromptsResult, ListToolsResult, PaginatedRequestParams, ProgressToken, RequestMetaObject,
    ServerCapabilities, ServerConfig, Tool,
};
use rmcp::{Peer, RoleServer, ServerHandler, service::RequestContext};
use serde_json::{Map, json};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

/// Tool that lists servers with deferred tools; only served by [`ToolScope::Deferred`].
pub const LIST_SERVERS_TOOL: &str = "_aether_list_servers";

const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_mins(10);

/// An rmcp [`ServerHandler`] that exposes the tools and prompts of every server behind
/// an [`McpHandle`] under `server__name`, forwarding calls to the owning server.
///
/// It holds no per-connection state, so one value can be cloned for every request
/// of a stateless transport.
#[derive(Clone)]
pub struct AggregateServer {
    handle: McpHandle,
    scope: ToolScope,
    server_info: Implementation,
    call_timeout: Duration,
}

/// Which tools an [`AggregateServer`] lists and how it routes calls to them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolScope {
    /// Every allowed tool, routed by its catalog exposure.
    All,
    /// Only deferred tools, plus [`LIST_SERVERS_TOOL`] for discovering their servers.
    Deferred,
}

impl AggregateServer {
    /// Serve every allowed tool.
    pub fn new(handle: McpHandle) -> Self {
        Self {
            handle,
            scope: ToolScope::All,
            server_info: Implementation::new("aether-mcp-aggregate", env!("CARGO_PKG_VERSION")),
            call_timeout: DEFAULT_CALL_TIMEOUT,
        }
    }

    /// Serve only deferred tools for progressive discovery.
    pub fn deferred(handle: McpHandle) -> Self {
        Self { scope: ToolScope::Deferred, ..Self::new(handle) }
    }

    pub fn with_server_info(mut self, server_info: Implementation) -> Self {
        self.server_info = server_info;
        self
    }

    pub fn with_call_timeout(mut self, timeout: Duration) -> Self {
        self.call_timeout = timeout;
        self
    }

    fn tools(&self) -> Vec<Tool> {
        let snapshot = self.handle.snapshot();
        let tools = snapshot.catalog().tools();
        let selected = match self.scope {
            ToolScope::All => [tools.model_visible, tools.deferred].concat(),
            ToolScope::Deferred => tools.deferred,
        };
        let mut listed: Vec<Tool> = selected.into_iter().map(CatalogTool::tool).cloned().collect();
        if self.scope == ToolScope::Deferred {
            listed.push(Tool::new(
                LIST_SERVERS_TOOL,
                "List connected MCP servers with deferred tools",
                Arc::new(Map::new()),
            ));
        }
        listed
    }

    fn route(&self, name: &str) -> Result<ToolRoute, ErrorData> {
        match self.scope {
            ToolScope::All => self
                .handle
                .snapshot()
                .catalog()
                .tool(name)
                .map(CatalogTool::route)
                .ok_or_else(|| ErrorData::invalid_params(format!("Tool not found: {name}"), None)),
            ToolScope::Deferred => split_on_server_name(name)
                .map(|(server, tool)| ToolRoute::Deferred { server: server.to_string(), tool: tool.to_string() })
                .ok_or_else(|| ErrorData::invalid_params("deferred tool names must use server__tool", None)),
        }
    }

    fn instructions(&self) -> Option<String> {
        if self.scope == ToolScope::Deferred {
            return None;
        }
        let instructions = self.handle.snapshot().model_instructions();
        (!instructions.is_empty()).then(|| {
            instructions
                .iter()
                .map(|(server, body)| format!("<mcp-server name=\"{server}\">\n{body}\n</mcp-server>"))
                .collect::<Vec<_>>()
                .join("\n\n")
        })
    }

    fn list_servers(&self) -> CallToolResult {
        let servers = self.handle.snapshot().catalog().discoverable_deferred_servers();
        let value = serde_json::to_value(
            servers
                .iter()
                .map(|server| json!({ "name": server.name, "description": server.description }))
                .collect::<Vec<_>>(),
        )
        .expect("server descriptions serialize");
        CallToolResult::structured(value)
    }
}

impl ServerHandler for AggregateServer {
    fn get_info(&self) -> ServerConfig {
        let config = ServerConfig::new(ServerCapabilities::builder().enable_tools().enable_prompts().build())
            .with_server_info(self.server_info.clone());
        match self.instructions() {
            Some(instructions) => config.with_instructions(instructions),
            None => config,
        }
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, ErrorData>> + Send + '_ {
        std::future::ready(Ok(ListToolsResult::with_all_items(self.tools())))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if self.scope == ToolScope::Deferred && request.name == LIST_SERVERS_TOOL {
            return Ok(self.list_servers().into());
        }
        let route = self.route(&request.name)?;
        let progress_token = context.meta.get_progress_token();
        let cancellation = CancellationToken::new();
        let guard = cancellation.clone().drop_guard();
        let started = Instant::now();
        let events = self.handle.call(
            route,
            request.arguments.unwrap_or_default(),
            CallToolOptions {
                timeout: self.call_timeout,
                meta: forwarded_meta(&context.meta),
                cancel: cancellation.clone(),
            },
        );

        let response = forward_events(events, &context, progress_token, &cancellation).await;
        guard.disarm();
        let outcome = match &response {
            Ok(_) => "success",
            Err(_) if cancellation.is_cancelled() => "cancelled",
            Err(_) => "error",
        };
        tracing::info!(
            tool = %request.name,
            outcome,
            duration_ms = started.elapsed().as_millis(),
            "aggregate MCP tool call completed"
        );
        response
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        let prompts = self.handle.list_prompts().await.map_err(prompt_error)?;
        Ok(ListPromptsResult::with_all_items(prompts))
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, ErrorData> {
        let prompt = self.handle.get_prompt(&request.name, request.arguments).await.map_err(prompt_error)?;
        Ok(GetPromptResponse::Complete(prompt))
    }
}

async fn forward_events(
    mut events: crate::client::ToolCallStream,
    context: &RequestContext<RoleServer>,
    progress_token: Option<ProgressToken>,
    cancellation: &CancellationToken,
) -> Result<CallToolResponse, ErrorData> {
    let disconnected = transport_closed(context.peer.clone());
    tokio::pin!(disconnected);
    loop {
        let event = tokio::select! {
            event = events.next() => event,
            () = context.ct.cancelled() => None,
            () = &mut disconnected => None,
        };
        let Some(event) = event else {
            cancellation.cancel();
            return Err(ErrorData::internal_error("tool call was cancelled", None));
        };
        match event {
            ToolCallEvent::Complete(result) | ToolCallEvent::TaskComplete { result, .. } => {
                return result.map(Into::into).map_err(Into::into);
            }
            ToolCallEvent::Cancelled { .. } => {
                return Err(ErrorData::internal_error("tool call was cancelled", None));
            }
            ToolCallEvent::Progress(mut progress) => {
                if let Some(token) = &progress_token {
                    progress.progress_token = token.clone();
                    let _ = context.peer.notify_progress(progress).await;
                }
            }
            ToolCallEvent::TaskCreated(_) | ToolCallEvent::TaskStatus(_) => {}
        }
    }
}

/// rmcp leaves in-flight request tokens uncancelled when a transport closes, so
/// a dropped caller is detected by polling the peer.
async fn transport_closed(peer: Peer<RoleServer>) {
    while !peer.is_transport_closed() {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Caller `_meta` (e.g. W3C trace context) minus the keys the upstream client sets itself.
fn forwarded_meta(meta: &RequestMetaObject) -> Option<RequestMetaObject> {
    let mut forwarded = meta.clone();
    forwarded.0.0.retain(|key, _| key != "progressToken" && !key.starts_with("io.modelcontextprotocol/"));
    (!forwarded.0.0.is_empty()).then_some(forwarded)
}

fn prompt_error(error: McpHandleError) -> ErrorData {
    match error {
        McpHandleError::Route(error) => ErrorData::invalid_params(error.to_string(), None),
        error => ErrorData::internal_error(error.to_string(), None),
    }
}
