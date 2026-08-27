use super::McpHandle;
use futures::StreamExt;
use mcp_utils::client::{CallToolOptions, CancellationToken, ToolCallEvent, ToolRoute};
use mcp_utils::tool_gateway::LIST_SERVERS_TOOL;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ErrorData, Implementation, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool, ToolAnnotations,
};
use rmcp::{RoleServer, ServerHandler, service::RequestContext};
use serde_json::{Map, json};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Clone)]
pub struct GatewayService {
    handle: McpHandle,
}

impl GatewayService {
    pub fn new(handle: McpHandle) -> Self {
        Self { handle }
    }

    fn tools(&self) -> Vec<Tool> {
        let snapshot = self.handle.snapshot();
        let mut tools = snapshot
            .catalog()
            .tools()
            .deferred
            .into_iter()
            .map(|tool| {
                let definition = tool.definition();
                let schema = definition.parameters.as_object().cloned().unwrap_or_default();
                let mut result = Tool::new(definition.name.clone(), definition.description.clone(), Arc::new(schema));
                if let Some(annotations) = &definition.annotations {
                    let mut converted = ToolAnnotations::new();
                    converted.title.clone_from(&annotations.title);
                    converted.read_only_hint = annotations.read_only_hint;
                    converted.destructive_hint = annotations.destructive_hint;
                    converted.idempotent_hint = annotations.idempotent_hint;
                    converted.open_world_hint = annotations.open_world_hint;
                    result = result.with_annotations(converted);
                }
                result
            })
            .collect::<Vec<_>>();
        tools.push(Tool::new(
            LIST_SERVERS_TOOL,
            "List connected MCP servers with deferred tools",
            Arc::new(Map::new()),
        ));
        tools
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

impl ServerHandler for GatewayService {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("aether-deferred-tool-gateway", env!("CARGO_PKG_VERSION")))
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, ErrorData>> + Send + '_ {
        std::future::ready(Ok(ListToolsResult::with_all_items(self.tools())))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if request.name == LIST_SERVERS_TOOL {
            return Ok(self.list_servers().into());
        }

        let (server, tool) = request
            .name
            .split_once("__")
            .ok_or_else(|| ErrorData::invalid_params("deferred tool names must use server__tool", None))?;

        let arguments = request.arguments.unwrap_or_default();
        let cancellation = CancellationToken::new();
        let mut guard = CallCancellationGuard::new(cancellation.clone());
        let started = Instant::now();
        let mut events = self.handle.call(
            ToolRoute::Deferred { server: server.to_string(), tool: tool.to_string() },
            arguments,
            CallToolOptions { timeout: Duration::from_mins(10), meta: request.meta, cancel: cancellation.clone() },
        );
        let peer = context.peer;
        let disconnected = async move {
            while !peer.is_transport_closed() {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        };
        tokio::pin!(disconnected);

        loop {
            let event = tokio::select! {
                event = events.next() => event,
                () = context.ct.cancelled() => None,
                () = &mut disconnected => None,
            };
            let Some(event) = event else {
                cancellation.cancel();
                tracing::info!(
                    route = "deferred",
                    server,
                    tool,
                    outcome = "cancelled",
                    duration_ms = started.elapsed().as_millis(),
                    "deferred MCP gateway call completed"
                );
                guard.disarm();
                return Err(ErrorData::internal_error("deferred tool call was cancelled", None));
            };
            match event {
                ToolCallEvent::Complete(result) | ToolCallEvent::TaskComplete { result, .. } => {
                    let outcome = if result.is_ok() { "success" } else { "error" };
                    tracing::info!(
                        route = "deferred",
                        server,
                        tool,
                        outcome,
                        duration_ms = started.elapsed().as_millis(),
                        "deferred MCP gateway call completed"
                    );
                    guard.disarm();
                    return result.map(Into::into).map_err(|error| ErrorData::internal_error(error.to_string(), None));
                }
                ToolCallEvent::Cancelled { .. } => {
                    tracing::info!(
                        route = "deferred",
                        server,
                        tool,
                        outcome = "cancelled",
                        duration_ms = started.elapsed().as_millis(),
                        "deferred MCP gateway call completed"
                    );
                    guard.disarm();
                    return Err(ErrorData::internal_error("deferred tool call was cancelled", None));
                }
                ToolCallEvent::Progress(_) | ToolCallEvent::TaskCreated(_) | ToolCallEvent::TaskStatus(_) => {}
            }
        }
    }
}

struct CallCancellationGuard {
    token: CancellationToken,
    armed: bool,
}

impl CallCancellationGuard {
    fn new(token: CancellationToken) -> Self {
        Self { token, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for CallCancellationGuard {
    fn drop(&mut self) {
        if self.armed {
            self.token.cancel();
        }
    }
}
