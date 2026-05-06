use aether_core::mcp::run_mcp_task::{McpCommand, ToolExecutionEvent};
use aether_core::mcp::{McpSpawnResult, mcp};
use mcp_utils::client::{McpServer, McpTransport};
use rmcp::{
    RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResult, CreateElicitationRequestParams, ElicitationAction, ErrorCode, ErrorData,
        Implementation, ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool,
    },
    service::{DynService, RequestContext},
};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// Fake MCP server whose single tool always fails with `-32042`
/// `URL_ELICITATION_REQUIRED`, carrying one URL elicitation request in `data`.
#[derive(Clone, Default)]
struct UrlElicitationRequiredServer {
    elicitation_id: String,
    url: String,
}

impl UrlElicitationRequiredServer {
    fn new(elicitation_id: impl Into<String>, url: impl Into<String>) -> Self {
        Self { elicitation_id: elicitation_id.into(), url: url.into() }
    }

    fn into_dyn(self) -> Box<dyn DynService<RoleServer>> {
        Box::new(self)
    }
}

#[derive(Clone, Default)]
struct MalformedUrlElicitationRequiredServer {
    data: serde_json::Value,
}

impl MalformedUrlElicitationRequiredServer {
    fn new(data: serde_json::Value) -> Self {
        Self { data }
    }

    fn into_dyn(self) -> Box<dyn DynService<RoleServer>> {
        Box::new(self)
    }
}

impl ServerHandler for UrlElicitationRequiredServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("url-elicit-required", "0.1.0"))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let input_schema = serde_json::from_value(json!({
            "type": "object",
            "properties": {}
        }))
        .unwrap();
        Ok(ListToolsResult {
            tools: vec![Tool::new("needs_browser", "Always returns URL_ELICITATION_REQUIRED", Arc::new(input_schema))],
            next_cursor: None,
            meta: None,
        })
    }

    async fn call_tool(
        &self,
        _request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let data = json!({
            "elicitations": [
                {
                    "mode": "url",
                    "message": "Authorize to continue",
                    "url": self.url,
                    "elicitationId": self.elicitation_id,
                }
            ]
        });
        Err(ErrorData::new(ErrorCode::URL_ELICITATION_REQUIRED, "browser interaction required", Some(data)))
    }
}

impl ServerHandler for MalformedUrlElicitationRequiredServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("malformed-url-elicit-required", "0.1.0"))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let input_schema = serde_json::from_value(json!({
            "type": "object",
            "properties": {}
        }))
        .unwrap();
        Ok(ListToolsResult {
            tools: vec![Tool::new(
                "needs_browser",
                "Returns malformed URL_ELICITATION_REQUIRED data",
                Arc::new(input_schema),
            )],
            next_cursor: None,
            meta: None,
        })
    }

    async fn call_tool(
        &self,
        _request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        Err(ErrorData::new(
            ErrorCode::URL_ELICITATION_REQUIRED,
            "browser interaction required",
            Some(self.data.clone()),
        ))
    }
}

fn fake_url_elicit_mcp(name: &str, server: UrlElicitationRequiredServer) -> McpServer {
    McpServer::new(name, McpTransport::InMemory { server: server.into_dyn() }, false)
}

async fn drain_until_complete(
    event_rx: &mut mpsc::Receiver<ToolExecutionEvent>,
) -> (Result<llm::ToolCallResult, llm::ToolCallError>, Option<mcp_utils::display_meta::ToolResultMeta>) {
    while let Some(event) = event_rx.recv().await {
        if let ToolExecutionEvent::Complete { result, result_meta, .. } = event {
            return (result, result_meta);
        }
    }
    panic!("event stream ended without Complete");
}

fn call_tool_request() -> llm::ToolCallRequest {
    llm::ToolCallRequest {
        id: "url-test-1".to_string(),
        name: "browser_server__needs_browser".to_string(),
        arguments: "{}".to_string(),
    }
}

/// What the scripting task captured from the elicitation request sent by the
/// manager. Includes the server name and the raw request params for assertion.
struct CapturedElicitation {
    server_name: String,
    request: CreateElicitationRequestParams,
}

/// Spawn an MCP manager with one fake server that always returns
/// `URL_ELICITATION_REQUIRED`, and wire up a task that scripts the user
/// response to any incoming elicitation request.
async fn spawn_scripted(
    elicitation_id: &str,
    url: &str,
    user_action: ElicitationAction,
) -> (mpsc::Sender<McpCommand>, tokio::task::JoinHandle<Option<CapturedElicitation>>) {
    let server = UrlElicitationRequiredServer::new(elicitation_id, url);
    let config = fake_url_elicit_mcp("browser_server", server);

    let McpSpawnResult { command_tx, mut event_rx, .. } = mcp().with_servers(vec![config]).spawn().await.unwrap();

    let script_handle = tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            if let mcp_utils::client::McpClientEvent::Elicitation(req) = event {
                let captured = CapturedElicitation { server_name: req.server_name, request: req.request };
                let _ = req.response_sender.send(rmcp::model::CreateElicitationResult {
                    action: user_action,
                    content: None,
                    meta: Option::default(),
                });
                return Some(captured);
            }
        }
        None
    });

    (command_tx, script_handle)
}

#[tokio::test]
async fn malformed_url_elicitation_required_data_returns_protocol_error() {
    let server = MalformedUrlElicitationRequiredServer::new(json!({ "elicitations": "not-an-array" }));
    let config = McpServer::new("browser_server", McpTransport::InMemory { server: server.into_dyn() }, false);

    let McpSpawnResult { command_tx, .. } = mcp().with_servers(vec![config]).spawn().await.unwrap();

    let (event_tx, mut event_rx) = mpsc::channel(10);
    command_tx
        .send(McpCommand::ExecuteTool { request: call_tool_request(), timeout: Duration::from_secs(10), tx: event_tx })
        .await
        .unwrap();

    let (result, _meta) = drain_until_complete(&mut event_rx).await;
    let err = result.expect_err("malformed payload should surface as a protocol error");
    assert!(
        err.error.contains("invalid") || err.error.contains("malformed"),
        "error should identify malformed URL elicitation payload: {}",
        err.error
    );
    assert!(!err.error.contains("https://"), "error should not leak URLs: {}", err.error);
}

#[tokio::test]
async fn url_elicitation_required_accept_returns_retry_needed_error_without_url() {
    let url = "https://github.com/login/oauth?elicitationId=el-42";
    let (command_tx, script_handle) = spawn_scripted("el-42", url, ElicitationAction::Accept).await;

    let (event_tx, mut event_rx) = mpsc::channel(10);
    command_tx
        .send(McpCommand::ExecuteTool { request: call_tool_request(), timeout: Duration::from_secs(10), tx: event_tx })
        .await
        .unwrap();

    let (result, _meta) = drain_until_complete(&mut event_rx).await;
    let err = result.expect_err("accept should still surface as tool error asking for retry");
    assert!(err.error.contains("browser flow"), "message should mention browser flow: {}", err.error);
    assert!(err.error.contains("Retry") || err.error.contains("retry"), "message should mention retry: {}", err.error);
    assert!(!err.error.contains("https://"), "URL must not leak into tool error text: {}", err.error);

    let captured = script_handle.await.unwrap().expect("elicitation was never dispatched");
    assert_eq!(captured.server_name, "browser_server");
    match captured.request {
        CreateElicitationRequestParams::UrlElicitationParams { url: req_url, elicitation_id, .. } => {
            assert_eq!(req_url, url);
            assert_eq!(elicitation_id, "el-42");
        }
        CreateElicitationRequestParams::FormElicitationParams { .. } => {
            panic!("expected UrlElicitationParams, got FormElicitationParams")
        }
    }
}

#[tokio::test]
async fn url_elicitation_required_decline_returns_decline_error_without_url() {
    let url = "https://example.com/auth";
    let (command_tx, _script_handle) = spawn_scripted("el-decl", url, ElicitationAction::Decline).await;

    let (event_tx, mut event_rx) = mpsc::channel(10);
    command_tx
        .send(McpCommand::ExecuteTool { request: call_tool_request(), timeout: Duration::from_secs(10), tx: event_tx })
        .await
        .unwrap();

    let (result, _meta) = drain_until_complete(&mut event_rx).await;
    let err = result.expect_err("decline should surface as tool error");
    assert!(err.error.contains("declined"), "message should mention decline: {}", err.error);
    assert!(!err.error.contains("https://"), "URL must not leak into tool error text: {}", err.error);
}

#[tokio::test]
async fn url_elicitation_required_cancel_returns_cancel_error_without_url() {
    let url = "https://example.com/auth";
    let (command_tx, _script_handle) = spawn_scripted("el-canc", url, ElicitationAction::Cancel).await;

    let (event_tx, mut event_rx) = mpsc::channel(10);
    command_tx
        .send(McpCommand::ExecuteTool { request: call_tool_request(), timeout: Duration::from_secs(10), tx: event_tx })
        .await
        .unwrap();

    let (result, _meta) = drain_until_complete(&mut event_rx).await;
    let err = result.expect_err("cancel should surface as tool error");
    assert!(err.error.contains("cancelled"), "message should mention cancel: {}", err.error);
    assert!(!err.error.contains("https://"), "URL must not leak into tool error text: {}", err.error);
}
