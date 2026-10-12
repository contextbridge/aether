use super::support::call_tool;
use aether_core::events::{AgentEvent, AgentObserver, TraceContext};
use aether_core::testing::test_agent;
use mcp_utils::testing::{FakeMcpServer, FakeMcpState, FakeTool, FakeToolResponse};
use serde_json::json;

#[tokio::test]
async fn tool_call_propagates_w3c_trace_context_in_request_metadata() {
    let trace_context = TraceContext {
        traceparent: "00-00112233445566778899aabbccddeeff-0123456789abcdef-01".to_string(),
        tracestate: Some("vendor=value".to_string()),
    };
    let (server, state) = capture_server();
    let agent = test_agent().mcp_server("trace", server).observer(Box::new(Tracing(trace_context.clone())));

    call_tool(agent, "trace__capture", json!({})).await.result().expect("capture tool succeeds");

    let calls = state.calls_for("capture");
    assert_eq!(calls[0].context_meta["traceparent"], json!(trace_context.traceparent));
    assert_eq!(calls[0].context_meta["tracestate"], json!("vendor=value"));
}

#[tokio::test]
async fn tool_call_without_trace_context_sends_no_trace_metadata() {
    let (server, state) = capture_server();

    call_tool(test_agent().mcp_server("trace", server), "trace__capture", json!({}))
        .await
        .result()
        .expect("capture tool succeeds");

    let calls = state.calls_for("capture");
    assert_eq!(calls[0].context_meta.get("traceparent"), None);
    assert_eq!(calls[0].context_meta.get("tracestate"), None);
}

struct Tracing(TraceContext);

impl AgentObserver for Tracing {
    fn on_event(&mut self, _message: &AgentEvent) {}

    fn tool_trace_context(&self, _tool_id: &str) -> Option<TraceContext> {
        Some(self.0.clone())
    }
}

fn capture_server() -> (FakeMcpServer, FakeMcpState) {
    let server = FakeMcpServer::new().with_tool(FakeTool::new("capture").responds(FakeToolResponse::text("captured")));
    let state = server.state();
    (server, state)
}
