use super::run_mcp_task::McpCommand;
use crate::events::TraceContext;
use llm::{ToolCallRequest, ToolDefinition};
use mcp_utils::client::{CancellationToken, ToolCallEvent};
use mcp_utils::tool_gateway::{LIST_SERVERS_TOOL, ServerDescription};
use rmcp::{
    RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ErrorData, GetPromptResult, Implementation,
        ListToolsResult, PaginatedRequestParams, Prompt, ServerCapabilities, ServerInfo, Tool, ToolAnnotations,
    },
    service::RequestContext,
};
use serde_json::{Map, Value, json};
use std::{sync::Arc, time::Duration};
use tokio::sync::{mpsc, oneshot};

#[derive(Clone)]
pub struct McpCommandClient {
    tx: mpsc::Sender<McpCommand>,
}

#[derive(Debug, thiserror::Error)]
pub enum McpCommandClientError {
    #[error("MCP manager is unavailable")]
    Unavailable,
    #[error("MCP operation failed: {0}")]
    Operation(String),
}

impl McpCommandClient {
    pub(crate) fn new(tx: mpsc::Sender<McpCommand>) -> Self {
        Self { tx }
    }

    pub fn unavailable() -> Self {
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        Self { tx }
    }

    pub async fn execute_tool(
        &self,
        request: ToolCallRequest,
        trace_context: Option<TraceContext>,
        timeout: Duration,
        tx: mpsc::Sender<ToolCallEvent>,
        cancel: CancellationToken,
    ) -> Result<(), McpCommandClientError> {
        self.tx
            .send(McpCommand::ExecuteTool { request, trace_context, timeout, tx, cancel })
            .await
            .map_err(|_| McpCommandClientError::Unavailable)
    }

    async fn deferred_servers(&self) -> Result<Vec<ServerDescription>, String> {
        let (reply, response) = oneshot::channel();
        self.tx
            .send(McpCommand::ListDeferredServers { reply })
            .await
            .map_err(|_| "MCP manager is unavailable".to_string())?;

        response.await.map_err(|_| "MCP manager is unavailable".to_string())
    }

    async fn deferred_tools(&self) -> Result<Vec<ToolDefinition>, String> {
        let (reply, response) = oneshot::channel();
        self.tx
            .send(McpCommand::ListDeferredTools { reply })
            .await
            .map_err(|_| "MCP manager is unavailable".to_string())?;
        response.await.map_err(|_| "MCP manager is unavailable".to_string())?
    }

    async fn call_deferred_tool(&self, request: CallToolRequestParams) -> Result<CallToolResult, String> {
        let (reply, response) = oneshot::channel();
        self.tx
            .send(McpCommand::ExecuteDeferredTool { request, reply })
            .await
            .map_err(|_| "MCP manager is unavailable".to_string())?;
        response.await.map_err(|_| "MCP manager is unavailable".to_string())?
    }

    pub async fn list_prompts(&self) -> Result<Vec<Prompt>, McpCommandClientError> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(McpCommand::ListPrompts { tx }).await.map_err(|_| McpCommandClientError::Unavailable)?;
        rx.await.map_err(|_| McpCommandClientError::Unavailable)?.map_err(McpCommandClientError::Operation)
    }

    pub async fn get_prompt(
        &self,
        name: String,
        arguments: Option<Map<String, Value>>,
    ) -> Result<GetPromptResult, McpCommandClientError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(McpCommand::GetPrompt { name, arguments, tx })
            .await
            .map_err(|_| McpCommandClientError::Unavailable)?;
        rx.await.map_err(|_| McpCommandClientError::Unavailable)?.map_err(McpCommandClientError::Operation)
    }

    pub async fn authenticate_server(&self, name: impl Into<String>) -> Result<(), McpCommandClientError> {
        self.tx
            .send(McpCommand::AuthenticateServer { name: name.into() })
            .await
            .map_err(|_| McpCommandClientError::Unavailable)
    }
}

impl ServerHandler for McpCommandClient {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("aether-tool-gateway", env!("CARGO_PKG_VERSION")))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let tools = self.deferred_tools().await.map_err(|error| ErrorData::internal_error(error, None))?;
        let mut result = Vec::with_capacity(tools.len() + 1);

        result.push(Tool::new(
            LIST_SERVERS_TOOL,
            "List deferred MCP servers available through this gateway",
            Arc::new(Map::from_iter([("type".into(), json!("object"))])),
        ));

        result.extend(tools.into_iter().map(tool_definition_to_mcp));
        Ok(ListToolsResult::with_all_items(result))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if request.name == LIST_SERVERS_TOOL {
            let servers = self.deferred_servers().await.map_err(|error| ErrorData::internal_error(error, None))?;
            let value =
                serde_json::to_value(servers).map_err(|error| ErrorData::internal_error(error.to_string(), None))?;
            return Ok(CallToolResult::structured(value).into());
        }

        self.call_deferred_tool(request).await.map(Into::into).map_err(|error| ErrorData::invalid_params(error, None))
    }
}

fn tool_definition_to_mcp(definition: ToolDefinition) -> Tool {
    let schema = definition.parameters.as_object().cloned().unwrap_or_default();
    let mut tool = Tool::new(definition.name, definition.description, Arc::new(schema));
    if let Some(annotations) = definition.annotations {
        tool = tool.with_annotations(ToolAnnotations::from_raw(
            annotations.title,
            annotations.read_only_hint,
            annotations.destructive_hint,
            annotations.idempotent_hint,
            annotations.open_world_hint,
        ));
    }
    tool
}
