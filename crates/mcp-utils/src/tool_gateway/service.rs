use crate::client::{CancellationToken, ToolCallEvent};
use crate::tool_gateway::LIST_SERVERS_TOOL;
use futures::{StreamExt, future::BoxFuture, stream::BoxStream};
use rmcp::{
    RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ErrorData, Implementation, ListToolsResult,
        PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
    },
    service::RequestContext,
};
use std::sync::Arc;

/// A request-scoped view of deferred tools and their completed-call driver.
pub trait GatewayBackend: Send + Sync + 'static {
    fn tools(&self, context: RequestContext<RoleServer>) -> BoxFuture<'_, Result<Vec<Tool>, ErrorData>>;
    fn servers(&self, context: RequestContext<RoleServer>) -> BoxFuture<'_, Result<CallToolResult, ErrorData>>;
    fn call(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
        cancel: CancellationToken,
    ) -> BoxStream<'static, ToolCallEvent>;
}

#[derive(Clone)]
pub struct GatewayService {
    backend: Arc<dyn GatewayBackend>,
}

impl GatewayService {
    pub fn new(backend: Arc<dyn GatewayBackend>) -> Self {
        Self { backend }
    }
}

impl ServerHandler for GatewayService {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("aether-deferred-tool-gateway", env!("CARGO_PKG_VERSION")))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let mut tools = self.backend.tools(context).await?;
        tools.push(Tool::new(LIST_SERVERS_TOOL, "List connected MCP servers with deferred tools", Arc::default()));
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if request.name == LIST_SERVERS_TOOL {
            return self.backend.servers(context).await.map(Into::into);
        }
        let cancellation = CancellationToken::new();
        let _guard = cancellation.clone().drop_guard();
        let mut events = self.backend.call(request, context.clone(), cancellation);
        let disconnected = async {
            while !context.peer.is_transport_closed() {
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
        };
        tokio::pin!(disconnected);
        loop {
            let event = tokio::select! {
                event = events.next() => event,
                () = context.ct.cancelled() => None,
                () = &mut disconnected => None,
            };
            match event {
                Some(ToolCallEvent::Complete(result) | ToolCallEvent::TaskComplete { result, .. }) => {
                    return result.map(Into::into).map_err(|error| ErrorData::internal_error(error.to_string(), None));
                }
                Some(ToolCallEvent::Progress(mut notification)) => {
                    if let Some(token) =
                        context.meta.get("progressToken").cloned().and_then(|value| serde_json::from_value(value).ok())
                    {
                        notification.progress_token = token;
                        let _ = context.peer.notify_progress(notification).await;
                    }
                }
                Some(ToolCallEvent::TaskCreated(_) | ToolCallEvent::TaskStatus(_)) => {}
                Some(ToolCallEvent::Cancelled { .. }) | None => {
                    return Err(ErrorData::internal_error("deferred tool call cancelled", None));
                }
            }
        }
    }
}
