use mcp_utils::client::{ClientOptions, McpClient, ToolCallOptions, Transport};
use mcp_utils::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ErrorData, ListToolsResult, PaginatedRequestParams,
    ServerCapabilities, ServerConfig, Tool,
};
use mcp_utils::server::{McpServer, ServerHandle};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, ServerHandler};
use serde_json::{Map, Value, json};
use std::fs::metadata;
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Notify;

#[tokio::test]
async fn serve_unix_binds_a_socket_in_a_private_directory() {
    let handle = McpServer::new(TestServer::default()).serve_unix().unwrap();

    let directory = handle.path().parent().unwrap();
    assert!(handle.path().exists());
    assert_eq!(metadata(directory).unwrap().permissions().mode() & 0o777, 0o700);
}

#[tokio::test]
async fn dropping_the_handle_removes_the_socket_and_its_directory() {
    let handle = McpServer::new(TestServer::default()).serve_unix().unwrap();
    let socket = handle.path().to_path_buf();
    let directory = socket.parent().unwrap().to_path_buf();

    drop(handle);

    assert!(!socket.exists());
    assert!(!directory.exists());
}

#[tokio::test]
async fn unix_clients_initialize_and_list_tools() {
    let handle = McpServer::new(TestServer::default()).serve_unix().unwrap();
    let client = connect(&handle).await;

    let tools = client.list_tools().await.unwrap();

    let names = tools.iter().map(|tool| tool.name.as_ref()).collect::<Vec<_>>();
    assert_eq!(names, TOOLS);
}

#[tokio::test]
async fn unix_clients_share_one_handler() {
    let handle = McpServer::new(TestServer::default()).serve_unix().unwrap();
    let first = connect(&handle).await;
    let second = connect(&handle).await;

    call(&first, "count").await;
    let result = call(&second, "count").await;

    assert_eq!(result.structured_content, Some(json!({"calls": 2})));
}

#[tokio::test]
async fn dropping_the_handle_cancels_in_flight_calls() {
    let server = TestServer::default();
    let slow_started = Arc::clone(&server.slow_started);
    let handle = McpServer::new(server).serve_unix().unwrap();
    let client = connect(&handle).await;
    let call =
        tokio::spawn(async move { client.call_tool("slow", Map::new(), ToolCallOptions::default()).result().await });
    slow_started.notified().await;

    drop(handle);

    assert!(call.await.unwrap().is_err());
}

#[tokio::test]
async fn closing_an_in_memory_client_stops_its_server() {
    let server = TestServer::default();
    let dropped = Arc::clone(&server.dropped);
    let transport = Transport::InProcess(McpServer::new(server));
    let client = McpClient::connect("test", transport, &ClientOptions::default()).await.unwrap();

    client.close().await;

    dropped.notified().await;
}

const TOOLS: [&str; 2] = ["count", "slow"];

#[derive(Default)]
struct TestServer {
    calls: AtomicUsize,
    slow_started: Arc<Notify>,
    dropped: Arc<Notify>,
}

impl ServerHandler for TestServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, ErrorData>> + Send + '_ {
        let tools = TOOLS.map(|name| Tool::new(name, name, Arc::new(Map::new())));
        std::future::ready(Ok(ListToolsResult::with_all_items(tools.to_vec())))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let result = match request.name.as_ref() {
            "count" => json!({"calls": self.calls.fetch_add(1, Ordering::SeqCst) + 1}),
            "slow" => {
                self.slow_started.notify_one();
                std::future::pending::<Value>().await
            }
            _ => return Err(ErrorData::invalid_params("unknown tool", None)),
        };
        Ok(CallToolResult::structured(result).into())
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.dropped.notify_one();
    }
}

async fn connect(handle: &ServerHandle) -> McpClient {
    McpClient::connect("test", Transport::Unix(handle.path().to_path_buf()), &ClientOptions::default()).await.unwrap()
}

async fn call(client: &McpClient, tool: &str) -> CallToolResult {
    client.call_tool(tool, Map::new(), ToolCallOptions::default()).result().await.unwrap()
}
