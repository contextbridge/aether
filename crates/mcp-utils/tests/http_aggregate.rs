//! Contract test for building a stateless HTTP MCP gateway from this crate's public API.

use axum::Router;
use futures::StreamExt;
use mcp_utils::aggregate::AggregateServer;
use mcp_utils::client::{
    CallToolOptions, McpClient, McpConfig, McpHandle, McpManager, McpSnapshot, ReconnectPolicy, RuntimeMcpServer,
    RuntimeMcpTransport, ToolCallEvent, ToolExposure, Vars, call_tool,
};
use mcp_utils::rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ClientCapabilities, ClientConfig, ErrorCode,
    Implementation,
};
use mcp_utils::rmcp::service::{RunningService, ServiceError};
use mcp_utils::rmcp::transport::StreamableHttpClientTransport;
use mcp_utils::rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use mcp_utils::rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::never::NeverSessionManager,
};
use mcp_utils::rmcp::{RoleClient, ServiceExt};
use mcp_utils::testing::{FakeMcpServer, FakeTool, FakeToolResponse, HttpTestServer};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use utils::mcp_status::McpServerStatus;

#[tokio::test]
async fn gateway_aggregates_header_authenticated_http_upstreams() {
    let report_schema = json!({"type": "object", "properties": {"report": {"type": "string"}}});
    let alpha = HttpTestServer::start(
        FakeMcpServer::new().with_tool(
            FakeTool::new("report")
                .title("Weekly report")
                .output_schema(report_schema.clone())
                .responds(FakeToolResponse::new(CallToolResult::structured(json!({"report": "ok"})))),
        ),
    )
    .await;
    let beta = HttpTestServer::start(
        FakeMcpServer::new()
            .with_tool(FakeTool::new("ping").responds(FakeToolResponse::text("pong").progress(1.0, Some(2.0)))),
    )
    .await;
    let config = json!({"mcpServers": {
        "alpha": {"type": "http", "url": alpha.url(), "headers": {"Authorization": "Bearer ${ALPHA_TOKEN}"}},
        "beta": {"type": "http", "url": beta.url(), "headers": {"X-Api-Key": "${BETA_KEY}"}}
    }});
    let vars = Vars::new().with("ALPHA_TOKEN", "alpha-secret").with("BETA_KEY", "beta-secret");
    let servers = McpConfig::from_json(&config.to_string())
        .unwrap()
        .into_servers(&vars)
        .unwrap()
        .into_iter()
        .map(RuntimeMcpServer::try_from)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let manager = McpManager::new().with_client_capabilities(ClientCapabilities::default());
    let handle = manager.handle();
    let _manager_task = manager.spawn(servers).await.unwrap();
    wait_until_connected(&handle, 2).await;

    let gateway = Gateway::serve(AggregateServer::new(handle.clone())).await;
    let client = gateway.connect().await;

    let tools = client.list_all_tools().await.unwrap();
    let names = tools.iter().map(|tool| tool.name.to_string()).collect::<Vec<_>>();
    assert!(names.contains(&"alpha__report".to_string()));
    assert!(names.contains(&"beta__ping".to_string()));
    let report = tools.iter().find(|tool| tool.name == "alpha__report").unwrap();
    assert_eq!(report.title.as_deref(), Some("Weekly report"));
    assert_eq!(report.output_schema.as_deref(), report_schema.as_object());

    let CallToolResponse::Complete(result) =
        client.call_tool_once(CallToolRequestParams::new("alpha__report")).await.unwrap()
    else {
        panic!("report completes");
    };
    assert_eq!(result.structured_content, Some(json!({"report": "ok"})));

    let events = call_tool(
        Arc::new(client),
        CallToolRequestParams::new("beta__ping"),
        CallToolOptions { timeout: Duration::from_secs(30), ..CallToolOptions::default() },
    )
    .collect::<Vec<_>>()
    .await;
    assert!(
        events.iter().any(|event| matches!(event, ToolCallEvent::Progress(progress) if progress.total == Some(2.0)))
    );
    assert!(matches!(events.last(), Some(ToolCallEvent::Complete(Ok(_)))));

    assert!(alpha.received_headers().iter().all(|headers| headers["authorization"] == "Bearer alpha-secret"));
    assert!(beta.received_headers().iter().all(|headers| headers["x-api-key"] == "beta-secret"));
    assert!(beta.received_headers().iter().all(|headers| !headers.contains_key("authorization")));
}

#[tokio::test]
async fn gateway_passes_upstream_errors_through_unchanged() {
    let upstream = HttpTestServer::start(FakeMcpServer::new().with_tool(FakeTool::new("unscripted"))).await;
    let handle = spawn_manager(McpManager::new(), vec![http_server("alpha", &upstream.url())]).await;
    wait_until_connected(&handle, 1).await;
    let gateway = Gateway::serve(AggregateServer::new(handle)).await;
    let client = gateway.connect().await;

    let error = client.call_tool_once(CallToolRequestParams::new("alpha__unscripted")).await.unwrap_err();

    let ServiceError::McpError(error) = error else { panic!("expected a JSON-RPC error, got {error:?}") };
    assert_eq!(error.code, ErrorCode::INVALID_PARAMS);
    assert!(error.message.contains("unknown tool: unscripted"));
}

#[tokio::test]
async fn failed_http_upstreams_reconnect_with_backoff() {
    let address = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
    let policy = ReconnectPolicy { initial_delay: Duration::from_millis(10), max_delay: Duration::from_millis(50) };
    let handle = spawn_manager(
        McpManager::new().with_reconnect(policy),
        vec![http_server("late", &format!("http://{address}/mcp"))],
    )
    .await;
    let mut snapshots = handle.subscribe();
    snapshots
        .wait_for(|snapshot| matches!(server_status(snapshot, "late"), Some(McpServerStatus::Failed { .. })))
        .await
        .unwrap();

    let _upstream = HttpTestServer::start_at(address, FakeMcpServer::new()).await;

    let snapshot = snapshots.wait_for(|snapshot| snapshot.catalog().tool("late__add_numbers").is_some()).await.unwrap();
    assert!(matches!(server_status(&snapshot, "late"), Some(McpServerStatus::Connected { .. })));
}

/// The downstream gateway: an [`AggregateServer`] behind a stateless streamable HTTP service.
struct Gateway {
    url: String,
    task: JoinHandle<()>,
}

impl Gateway {
    async fn serve(server: AggregateServer) -> Self {
        let service = StreamableHttpService::new(
            move || Ok(server.clone()),
            Arc::new(NeverSessionManager::default()),
            StreamableHttpServerConfig::default().with_legacy_session_mode(false),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let app = Router::new().route_service("/mcp", service);
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self { url, task }
    }

    async fn connect(&self) -> RunningService<RoleClient, McpClient> {
        let client = McpClient::new(
            ClientConfig::new(ClientCapabilities::default(), Implementation::new("gateway-test", "0.1.0")),
            "gateway".to_string(),
        );
        client.serve(StreamableHttpClientTransport::from_uri(self.url.as_str())).await.unwrap()
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn spawn_manager(manager: McpManager, servers: Vec<RuntimeMcpServer>) -> McpHandle {
    let handle = manager.handle();
    manager.spawn(servers).await.unwrap();
    handle
}

fn http_server(name: &str, url: &str) -> RuntimeMcpServer {
    let transport = RuntimeMcpTransport::Http(StreamableHttpClientTransportConfig::with_uri(url).into());
    RuntimeMcpServer::new(name, transport, ToolExposure::ModelVisible)
}

async fn wait_until_connected(handle: &McpHandle, servers: usize) {
    let mut snapshots: watch::Receiver<Arc<McpSnapshot>> = handle.subscribe();
    snapshots
        .wait_for(|snapshot| {
            let statuses = snapshot.server_statuses();
            statuses.len() == servers
                && statuses.iter().all(|entry| matches!(entry.status, McpServerStatus::Connected { .. }))
        })
        .await
        .unwrap();
}

fn server_status(snapshot: &McpSnapshot, name: &str) -> Option<McpServerStatus> {
    snapshot.server_statuses().into_iter().find(|entry| entry.name == name).map(|entry| entry.status)
}
