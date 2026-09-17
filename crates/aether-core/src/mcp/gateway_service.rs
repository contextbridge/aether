use super::McpHandle;
use futures::{future::BoxFuture, stream::BoxStream};
use mcp_utils::{
    client::{CallToolOptions, CancellationToken, CatalogTool, ToolCallEvent, ToolRoute},
    tool_gateway::service::{GatewayBackend, GatewayService as SharedGatewayService},
};
use rmcp::{
    RoleServer,
    model::{CallToolRequestParams, CallToolResult, ErrorData, Tool},
    service::RequestContext,
};
use serde_json::json;
use std::{sync::Arc, time::Duration};

pub struct GatewayService;

impl GatewayService {
    pub fn from_handle(handle: McpHandle) -> SharedGatewayService {
        SharedGatewayService::new(Arc::new(LocalGateway(handle)))
    }
}

struct LocalGateway(McpHandle);

impl GatewayBackend for LocalGateway {
    fn tools(&self, _context: RequestContext<RoleServer>) -> BoxFuture<'_, Result<Vec<Tool>, ErrorData>> {
        Box::pin(async move {
            Ok(self
                .0
                .snapshot()
                .catalog()
                .tools()
                .deferred
                .into_iter()
                .map(CatalogTool::namespaced_mcp_definition)
                .collect())
        })
    }

    fn servers(&self, _context: RequestContext<RoleServer>) -> BoxFuture<'_, Result<CallToolResult, ErrorData>> {
        Box::pin(async move {
            let servers = self.0.snapshot().catalog().discoverable_deferred_servers();
            Ok(CallToolResult::structured(json!(
                servers
                    .iter()
                    .map(|server| json!({"name":server.name,"description":server.description}))
                    .collect::<Vec<_>>()
            )))
        })
    }

    fn call(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
        cancel: CancellationToken,
    ) -> BoxStream<'static, ToolCallEvent> {
        let Some((server, tool)) = request.name.split_once("__") else {
            return Box::pin(futures::stream::once(async {
                ToolCallEvent::Complete(Err(mcp_utils::client::CallToolError::Unavailable {
                    message: "deferred tool names must use server__tool".into(),
                }))
            }));
        };
        self.0.call(
            ToolRoute::Deferred { server: server.to_string(), tool: tool.to_string() },
            request.arguments.unwrap_or_default(),
            CallToolOptions { timeout: Duration::from_mins(10), meta: Some(context.meta), cancel },
        )
    }
}
