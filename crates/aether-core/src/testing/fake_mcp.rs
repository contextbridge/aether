use rmcp::{
    ErrorData as McpError, Json, RoleServer, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Content, Implementation, ServerCapabilities, ServerInfo},
    service::DynService,
    tool, tool_handler, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use mcp_utils::client::{McpServer, McpTransport};

pub fn fake_mcp(name: &str, server: FakeMcpServer) -> McpServer {
    fake_mcp_with_proxy(name, server, false)
}

pub fn fake_mcp_with_proxy(name: &str, server: FakeMcpServer, proxy: bool) -> McpServer {
    McpServer::new(name, McpTransport::InMemory { server: server.into_dyn() }, proxy)
}

/// A fake MCP server for testing
#[derive(Clone)]
pub struct FakeMcpServer {
    tool_router: ToolRouter<Self>,
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for FakeMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(
                Implementation::new("fake-mcp-server", "0.1.0").with_description("A fake MCP server for testing"),
            )
            .with_instructions("A fake MCP server for testing")
    }
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct AddNumbersRequest {
    pub a: u32,
    pub b: u32,
}

impl AddNumbersRequest {
    pub fn new(a: u32, b: u32) -> Self {
        Self { a, b }
    }

    pub fn json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct AddNumbersResult {
    pub sum: u32,
}

impl AddNumbersResult {
    pub fn new(sum: u32) -> Self {
        Self { sum }
    }

    pub fn json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct DivideNumbersRequest {
    pub a: i32,
    pub b: i32,
}

impl DivideNumbersRequest {
    pub fn new(a: i32, b: i32) -> Self {
        Self { a, b }
    }

    pub fn json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct DivideNumbersResult {
    pub quotient: i32,
}

impl DivideNumbersResult {
    pub fn new(quotient: i32) -> Self {
        Self { quotient }
    }

    pub fn json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct SlowToolRequest {
    pub sleep_ms: u64,
}

impl SlowToolRequest {
    pub fn new(sleep_ms: u64) -> Self {
        Self { sleep_ms }
    }

    pub fn json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct SlowToolResult {
    pub message: String,
}

impl SlowToolResult {
    pub fn new(message: String) -> Self {
        Self { message }
    }

    pub fn json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

impl Default for FakeMcpServer {
    fn default() -> Self {
        Self { tool_router: Self::tool_router() }
    }
}

#[tool_router]
impl FakeMcpServer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn into_dyn(self) -> Box<dyn DynService<RoleServer>> {
        Box::new(self)
    }

    #[tool(description = "Adds two numbers together")]
    pub async fn add_numbers(&self, request: Parameters<AddNumbersRequest>) -> Json<AddNumbersResult> {
        let Parameters(AddNumbersRequest { a, b }) = request;
        Json(AddNumbersResult { sum: a + b })
    }

    #[tool(description = "Divides two numbers")]
    pub async fn divide_numbers(&self, request: Parameters<DivideNumbersRequest>) -> Result<CallToolResult, McpError> {
        let Parameters(DivideNumbersRequest { a, b }) = request;

        if b == 0 {
            return Ok(CallToolResult::error(vec![Content::text("Division by zero")]));
        }

        let result = DivideNumbersResult { quotient: a / b };
        let result_json = serde_json::to_string(&result).unwrap();

        Ok(CallToolResult::success(vec![Content::text(result_json)]))
    }

    #[tool(description = "A tool that sleeps for a specified duration (for testing timeouts)")]
    pub async fn slow_tool(&self, request: Parameters<SlowToolRequest>) -> Json<SlowToolResult> {
        let Parameters(SlowToolRequest { sleep_ms }) = request;
        tokio::time::sleep(std::time::Duration::from_millis(sleep_ms)).await;
        Json(SlowToolResult { message: format!("Slept for {sleep_ms}ms") })
    }
}
