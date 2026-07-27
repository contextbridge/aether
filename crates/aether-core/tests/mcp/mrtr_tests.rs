//! Integration tests for the client-side MRTR (multi round-trip request,
//! 2026-07-28 spec) handling in `run_mcp_task`.
//!
//! A remote server responds to `tools/call` with `InputRequiredResult`
//! (carrying an elicitation `inputRequest` and an opaque `requestState`).
//! The client surfaces the elicitation through its existing consent channel,
//! then retries the original call with the echoed `requestState` plus the
//! collected `inputResponses`, finally receiving a normal `CallToolResult`.

use aether_core::mcp::mcp;
use aether_core::mcp::run_mcp_task::{McpCommand, ToolExecutionEvent};
use mcp_utils::client::{McpServer, McpTransport};
use rmcp::{
    RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ElicitRequest, ElicitRequestParams,
        ElicitationSchema, Implementation, InputRequest, InputRequests, InputRequiredResult, ListToolsResult,
        PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool,
    },
    service::{DynService, RequestContext},
};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::mpsc;

const REQUEST_STATE: &str = "opaque-server-state-123";

/// Fake server that demands an elicitation on the first `tools/call` and
/// completes once the client retries with `inputResponses`.
#[derive(Clone)]
struct MrtrServer {
    request_state: String,
    calls: Arc<AtomicUsize>,
}

impl MrtrServer {
    fn new() -> Self {
        Self { request_state: REQUEST_STATE.to_string(), calls: Arc::new(AtomicUsize::new(0)) }
    }

    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn into_dyn(self) -> Box<dyn DynService<RoleServer>> {
        Box::new(self)
    }
}

impl ServerHandler for MrtrServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("mrtr-server", "0.1.0"))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, rmcp::ErrorData> {
        let schema = serde_json::from_value(json!({"type": "object", "properties": {}})).unwrap();
        Ok(ListToolsResult::with_all_items(vec![Tool::new(
            "needs_input",
            "Returns InputRequiredResult",
            Arc::new(schema),
        )]))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, rmcp::ErrorData> {
        self.calls.fetch_add(1, Ordering::SeqCst);

        // Retry: the client echoed requestState + inputResponses.
        if request.input_responses.is_some() && request.request_state.as_deref() == Some(REQUEST_STATE) {
            return Ok(CallToolResult::success(vec![ContentBlock::text("done after input")]).into());
        }

        // First call: request an elicitation before completing.
        let schema = ElicitationSchema::builder().required_string("value").build().unwrap();
        let elicitation = ElicitRequest::new(ElicitRequestParams::FormElicitationParams {
            meta: None,
            message: "Pick a value".to_string(),
            requested_schema: schema,
        });
        let mut input_requests = InputRequests::new();
        input_requests.insert("elicit-1".to_string(), InputRequest::Elicitation(elicitation));
        Ok(InputRequiredResult::new(Some(input_requests), Some(self.request_state.clone())).into())
    }
}

fn call_tool_request() -> llm::ToolCallRequest {
    llm::ToolCallRequest {
        id: "mrtr-1".to_string(),
        name: "mrtr_server__needs_input".to_string(),
        arguments: "{}".to_string(),
    }
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

async fn call_needs_input(command_tx: &mpsc::Sender<McpCommand>) -> Result<llm::ToolCallResult, llm::ToolCallError> {
    let (event_tx, mut event_rx) = mpsc::channel(10);
    command_tx
        .send(McpCommand::ExecuteTool { request: call_tool_request(), timeout: Duration::from_secs(10), tx: event_tx })
        .await
        .unwrap();
    drain_until_complete(&mut event_rx).await.0
}

/// Spawn the manager with `server`, scripting an `Accept` response carrying
/// `content` to the first elicitation that arrives. Returns the command sender
/// and the task that captures the surfaced elicitation.
async fn spawn_with_scripted_elicitation(
    server: MrtrServer,
    content: serde_json::Value,
) -> (mpsc::Sender<McpCommand>, tokio::task::JoinHandle<Option<(String, ElicitRequestParams)>>, MrtrServer) {
    let config = McpServer::new("mrtr_server", McpTransport::InMemory { server: server.clone().into_dyn() }, false);

    let mut spawn = mcp("/workspace").with_servers(vec![config]).spawn().await.unwrap();
    let _snapshot = spawn.block_until_ready().await.expect("bootstrap completes");
    let command_tx = spawn.command_tx;
    let mut event_rx = spawn.event_rx;

    let script = tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            if let mcp_utils::client::McpClientEvent::Elicitation(req) = event {
                let captured = (req.server_name.clone(), req.request.clone());
                let _ = req
                    .response_sender
                    .send(rmcp::model::ElicitResult::new(rmcp::model::ElicitationAction::Accept).with_content(content));
                return Some(captured);
            }
        }
        None
    });

    (command_tx, script, server)
}

#[tokio::test]
async fn input_required_result_surfaces_elicitation_and_retries_to_completion() {
    let (command_tx, script_handle, server) =
        spawn_with_scripted_elicitation(MrtrServer::new(), json!({"value": "green"})).await;

    let result = call_needs_input(&command_tx).await.expect("retry should complete the tool call");
    assert!(result.result.contains("done after input"), "result should contain final content: {}", result.result);

    // The server saw the InputRequired round and the echoed-state retry round.
    assert_eq!(server.call_count(), 2, "server should be called exactly twice (input required, then complete)");

    let (server_name, request) = script_handle.await.unwrap().expect("elicitation was never surfaced");
    assert_eq!(server_name, "mrtr_server");
    assert!(matches!(request, ElicitRequestParams::FormElicitationParams { .. }), "expected a form elicitation");
}

/// A server that never completes — it keeps returning `InputRequiredResult` —
/// must hit the client-side retry cap instead of looping forever.
#[derive(Clone)]
struct AlwaysInputRequiredServer;

impl ServerHandler for AlwaysInputRequiredServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("always-mrtr", "0.1.0"))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, rmcp::ErrorData> {
        let schema = serde_json::from_value(json!({"type": "object", "properties": {}})).unwrap();
        Ok(ListToolsResult::with_all_items(vec![Tool::new("needs_input", "Always InputRequired", Arc::new(schema))]))
    }

    async fn call_tool(
        &self,
        _request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, rmcp::ErrorData> {
        let schema = ElicitationSchema::builder().required_string("value").build().unwrap();
        let elicitation = ElicitRequest::new(ElicitRequestParams::FormElicitationParams {
            meta: None,
            message: "Pick a value".to_string(),
            requested_schema: schema,
        });
        let mut input_requests = InputRequests::new();
        input_requests.insert("elicit-1".to_string(), InputRequest::Elicitation(elicitation));
        Ok(InputRequiredResult::new(Some(input_requests), Some(REQUEST_STATE.to_string())).into())
    }
}

#[tokio::test]
async fn repeated_input_required_hits_retry_cap() {
    let config =
        McpServer::new("mrtr_server", McpTransport::InMemory { server: Box::new(AlwaysInputRequiredServer) }, false);

    let mut spawn = mcp("/workspace").with_servers(vec![config]).spawn().await.unwrap();
    let _snapshot = spawn.block_until_ready().await.expect("bootstrap completes");
    let mut event_rx = spawn.event_rx;

    // Script the elicitation so the client keeps retrying until the cap.
    let script = tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            if let mcp_utils::client::McpClientEvent::Elicitation(req) = event {
                let _ = req.response_sender.send(
                    rmcp::model::ElicitResult::new(rmcp::model::ElicitationAction::Accept)
                        .with_content(json!({"value": "x"})),
                );
            }
        }
    });

    let result = call_needs_input(&spawn.command_tx).await;
    drop(script);
    let err = result.expect_err("repeated input required should hit the retry cap");
    assert!(
        err.error.contains("more than") && err.error.contains("times"),
        "error should mention the retry cap: {}",
        err.error
    );
}
