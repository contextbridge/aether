use mcp_utils::ServiceExt;
use mcp_utils::tool_gateway::{ToolGatewayEndpointParseError, UnixSocketMcpTransport, UnixSocketPath};
use rmcp::{
    RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ErrorData, Implementation, ListToolsResult,
        PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool,
    },
    service::RequestContext,
};
use serde_json::{Map, json};
use std::ffi::OsString;
use std::sync::Arc;

#[tokio::test]
async fn concurrent_runtimes_use_unique_sockets_in_the_same_runtime_directory() {
    let directory = tempfile::tempdir().unwrap();
    let first = UnixSocketMcpTransport::bind_in(directory.path()).unwrap();
    let second = UnixSocketMcpTransport::bind_in(directory.path()).unwrap();

    assert_ne!(first.endpoint().socket_path(), second.endpoint().socket_path());
}

#[tokio::test]
async fn unix_socket_serves_mcp_tools() {
    let gateway = UnixSocketMcpTransport::bind().unwrap();
    let endpoint = gateway.endpoint();
    let _handle = gateway.start(FakeGateway);
    let client = ().serve(tokio::net::UnixStream::connect(endpoint.socket_path()).await.unwrap()).await.unwrap();

    let tools = client.peer().list_all_tools().await.unwrap();
    assert_eq!(tools[0].name, "fake__echo");

    let result = client
        .peer()
        .call_tool(
            CallToolRequestParams::new("fake__echo").with_arguments(Map::from_iter([("value".into(), json!("hello"))])),
        )
        .await
        .unwrap();
    assert_eq!(result.structured_content, Some(json!({"value": "hello"})));
}

#[test]
fn tool_gateway_endpoint_rejects_malformed_values() {
    assert_eq!(
        UnixSocketPath::parse(OsString::from("relative.sock")),
        Err(ToolGatewayEndpointParseError::InvalidSocketPath)
    );
}

#[derive(Clone)]
struct FakeGateway;

impl ServerHandler for FakeGateway {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("fake-gateway", "1"))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(vec![Tool::new(
            "fake__echo",
            "Echo a value",
            Arc::new(Map::from_iter([("type".into(), json!("object"))])),
        )]))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        Ok(CallToolResult::structured(json!(request.arguments.unwrap_or_default())).into())
    }
}
