use aether_core::events::TraceContext;
use aether_core::mcp::mcp;
use aether_core::mcp::run_mcp_task::{McpCommand, ToolExecutionEvent};
use mcp_utils::client::{McpServer, McpTransport};
use rmcp::{
    RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResult, ErrorData, Implementation, ListToolsResult, PaginatedRequestParams,
        ServerCapabilities, ServerInfo, Tool,
    },
    service::{DynService, RequestContext},
};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

#[tokio::test]
async fn tool_call_propagates_w3c_trace_context_in_request_metadata() {
    let trace_context = TraceContext {
        traceparent: "00-00112233445566778899aabbccddeeff-0123456789abcdef-01".to_string(),
        tracestate: Some("vendor=value".to_string()),
    };

    let meta = captured_request_meta(Some(trace_context.clone())).await;
    assert_eq!(meta["traceparent"], json!(trace_context.traceparent));
    assert_eq!(meta["tracestate"], json!("vendor=value"));
}

#[tokio::test]
async fn tool_call_without_trace_context_sends_no_trace_metadata() {
    let meta = captured_request_meta(None).await;
    assert_eq!(meta.get("traceparent"), None);
    assert_eq!(meta.get("tracestate"), None);
}

/// Fake MCP server whose single tool echoes the request's metadata back as
/// structured content, so tests can assert exactly what arrived at the server.
#[derive(Clone, Default)]
struct MetaEchoServer;

impl MetaEchoServer {
    fn into_dyn(self) -> Box<dyn DynService<RoleServer>> {
        Box::new(self)
    }
}

impl ServerHandler for MetaEchoServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("meta-echo", "0.1.0"))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let input_schema = serde_json::from_value(json!({ "type": "object", "properties": {} })).unwrap();
        Ok(ListToolsResult {
            tools: vec![Tool::new("capture", "Echoes request metadata", Arc::new(input_schema))],
            next_cursor: None,
            meta: None,
        })
    }

    async fn call_tool(
        &self,
        _request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        Ok(CallToolResult::structured(serde_json::Value::Object(context.meta.0.clone())))
    }
}

/// Executes the capture tool with `trace_context` attached and returns the
/// request metadata the server saw, parsed from the echoed tool result.
async fn captured_request_meta(trace_context: Option<TraceContext>) -> serde_json::Value {
    let config = McpServer::new("trace", McpTransport::InMemory { server: MetaEchoServer.into_dyn() }, false);
    let mut spawn = mcp("/workspace").with_servers(vec![config]).spawn().await.unwrap();
    let _snapshot = spawn.block_until_ready().await.expect("bootstrap completes");
    let request = llm::ToolCallRequest {
        id: "trace-test-1".to_string(),
        name: "trace__capture".to_string(),
        arguments: "{}".to_string(),
    };

    let (event_tx, mut event_rx) = mpsc::channel(10);
    spawn
        .command_tx
        .send(McpCommand::ExecuteTool { request, trace_context, timeout: Duration::from_secs(10), tx: event_tx })
        .await
        .unwrap();

    let result = drain_until_complete(&mut event_rx).await.expect("capture tool succeeds");
    serde_yml::from_str(&result.result).expect("tool result is the echoed metadata")
}

async fn drain_until_complete(
    event_rx: &mut mpsc::Receiver<ToolExecutionEvent>,
) -> Result<llm::ToolCallResult, llm::ToolCallError> {
    while let Some(event) = event_rx.recv().await {
        if let ToolExecutionEvent::Complete { result, .. } = event {
            return result;
        }
    }
    panic!("event stream ended without Complete");
}
