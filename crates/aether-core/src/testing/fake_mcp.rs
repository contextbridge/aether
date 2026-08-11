use mcp_utils::client::{McpServer, McpTransport};
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    model::{
        CacheScope, CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
        ListToolsResult, PaginatedRequestParams, ProgressNotificationParam, ProtocolVersion, ResultType,
        ServerCapabilities, ServerInfo, Tool,
    },
    service::{DynService, RequestContext},
};
use serde_json::json;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub fn fake_mcp(name: &str, server: FakeMcpServer) -> McpServer {
    fake_mcp_with_proxy(name, server, false)
}

pub fn fake_mcp_with_proxy(name: &str, server: FakeMcpServer, proxy: bool) -> McpServer {
    McpServer::new(name, McpTransport::InMemory { server: server.into_dyn() }, proxy)
}

/// A fake MCP server preloaded with the classic math tools (`add_numbers`,
/// `divide_numbers`, `slow_tool`); add scripted tools with [`Self::with_tool`].
#[derive(Clone)]
pub struct FakeMcpServer {
    state: FakeMcpState,
}

#[derive(Clone, Default)]
pub struct FakeMcpState {
    inner: Arc<Mutex<FakeMcpStateInner>>,
}

#[derive(Clone)]
pub struct CapturedToolCall {
    pub request: CallToolRequestParams,
    pub context_meta: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone)]
pub struct FakeTool {
    definition: Tool,
    responses: HashMap<Option<String>, FakeToolResponse>,
    handler: Option<ToolHandler>,
}

#[derive(Clone)]
pub struct FakeToolResponse {
    response: CallToolResponse,
    delay: Duration,
    progress: Vec<(f64, Option<f64>)>,
}

impl FakeMcpServer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_tool(self, tool: FakeTool) -> Self {
        self.state.add_tool(tool);
        self
    }

    pub fn state(&self) -> FakeMcpState {
        self.state.clone()
    }

    pub fn into_dyn(self) -> Box<dyn DynService<RoleServer>> {
        Box::new(self)
    }
}

impl FakeMcpState {
    pub fn calls_for(&self, tool: &str) -> Vec<CapturedToolCall> {
        self.lock().calls.iter().filter(|call| call.request.name.as_ref() == tool).cloned().collect()
    }

    fn add_tool(&self, tool: FakeTool) {
        self.lock().tools.insert(tool.definition.name.to_string(), tool);
    }

    fn definitions(&self) -> Vec<Tool> {
        self.lock().tools.values().map(|tool| tool.definition.clone()).collect()
    }

    fn response_for(
        &self,
        request: &CallToolRequestParams,
        context_meta: serde_json::Map<String, serde_json::Value>,
    ) -> Option<FakeToolResponse> {
        let mut inner = self.lock();
        inner.calls.push(CapturedToolCall { request: request.clone(), context_meta });
        inner.tools.get(request.name.as_ref()).and_then(|tool| tool.response_for(request))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, FakeMcpStateInner> {
        self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl FakeTool {
    pub fn new(name: impl Into<String>) -> Self {
        let name = name.into();
        let schema = serde_json::from_value(json!({ "type": "object", "properties": {} }))
            .expect("empty object schema is valid");
        Self {
            definition: Tool::new(name, "Fake MCP tool", Arc::new(schema)),
            responses: HashMap::new(),
            handler: None,
        }
    }

    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.definition.description = Some(description.into().into());
        self
    }

    pub fn responds(mut self, response: impl Into<FakeToolResponse>) -> Self {
        self.responses.insert(None, response.into());
        self
    }

    pub fn when_state(mut self, state: impl Into<String>, response: impl Into<FakeToolResponse>) -> Self {
        self.responses.insert(Some(state.into()), response.into());
        self
    }

    /// Compute the response from the request, for tools whose output depends
    /// on their arguments. Scripted responses take precedence.
    pub fn responds_with(
        mut self,
        handler: impl Fn(&CallToolRequestParams) -> FakeToolResponse + Send + Sync + 'static,
    ) -> Self {
        self.handler = Some(Arc::new(handler));
        self
    }

    fn response_for(&self, request: &CallToolRequestParams) -> Option<FakeToolResponse> {
        self.responses
            .get(&request.request_state.as_deref().map(str::to_string))
            .cloned()
            .or_else(|| self.handler.as_ref().map(|handler| handler(request)))
    }
}

impl FakeToolResponse {
    pub fn new(response: impl Into<CallToolResponse>) -> Self {
        Self { response: response.into(), delay: Duration::ZERO, progress: Vec::new() }
    }

    pub fn text(text: impl Into<String>) -> Self {
        Self::new(CallToolResult::success(vec![ContentBlock::text(text.into())]))
    }

    pub fn delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    pub fn progress(mut self, progress: f64, total: Option<f64>) -> Self {
        self.progress.push((progress, total));
        self
    }
}

impl<T> From<T> for FakeToolResponse
where
    T: Into<CallToolResponse>,
{
    fn from(response: T) -> Self {
        Self::new(response)
    }
}

impl Default for FakeMcpServer {
    fn default() -> Self {
        Self { state: FakeMcpState::default() }
            .with_tool(add_numbers())
            .with_tool(divide_numbers())
            .with_tool(slow_tool())
    }
}

impl ServerHandler for FakeMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(
                Implementation::new("fake-mcp-server", "0.1.0").with_description("A fake MCP server for testing"),
            )
            .with_instructions("A fake MCP server for testing")
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let supports_cache_hints =
            context.protocol_version().is_some_and(|version| version >= ProtocolVersion::V_2026_07_28);
        Ok(ListToolsResult {
            result_type: Some(ResultType::COMPLETE),
            tools: self.state.definitions(),
            meta: None,
            next_cursor: None,
            ttl_ms: supports_cache_hints.then_some(0),
            cache_scope: supports_cache_hints.then_some(CacheScope::Public),
        })
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.state.definitions().into_iter().find(|tool| tool.name == name)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let response = self.state.response_for(&request, context.meta.0.0.clone());
        let Some(response) = response else {
            return Err(McpError::invalid_params(format!("unknown tool: {}", request.name), None));
        };

        if !response.delay.is_zero() {
            tokio::time::sleep(response.delay).await;
        }
        if let Some(token) = context.meta.get_progress_token() {
            for (progress, total) in response.progress {
                let mut notification = ProgressNotificationParam::new(token.clone(), progress);
                if let Some(total) = total {
                    notification = notification.with_total(total);
                }
                let _ = context.peer.notify_progress(notification).await;
            }
        }
        Ok(response.response)
    }
}

type ToolHandler = Arc<dyn Fn(&CallToolRequestParams) -> FakeToolResponse + Send + Sync>;

#[derive(Default)]
struct FakeMcpStateInner {
    tools: BTreeMap<String, FakeTool>,
    calls: Vec<CapturedToolCall>,
}

fn add_numbers() -> FakeTool {
    FakeTool::new("add_numbers").description("Adds two numbers together").responds_with(|request| {
        let sum = int_arg(request, "a") + int_arg(request, "b");
        FakeToolResponse::new(CallToolResult::structured(json!({ "sum": sum })))
    })
}

fn divide_numbers() -> FakeTool {
    FakeTool::new("divide_numbers").description("Divides two numbers").responds_with(|request| {
        let (a, b) = (int_arg(request, "a"), int_arg(request, "b"));
        if b == 0 {
            return FakeToolResponse::new(CallToolResult::error(vec![ContentBlock::text("Division by zero")]));
        }
        FakeToolResponse::new(CallToolResult::structured(json!({ "quotient": a / b })))
    })
}

fn slow_tool() -> FakeTool {
    FakeTool::new("slow_tool")
        .description("A tool that sleeps for a specified duration (for testing timeouts)")
        .responds_with(|request| {
            let sleep_ms = int_arg(request, "sleep_ms").unsigned_abs();
            FakeToolResponse::new(CallToolResult::structured(json!({ "message": format!("Slept for {sleep_ms}ms") })))
                .delay(Duration::from_millis(sleep_ms))
        })
}

fn int_arg(request: &CallToolRequestParams, name: &str) -> i64 {
    request.arguments.as_ref().and_then(|args| args.get(name)).and_then(serde_json::Value::as_i64).unwrap_or_default()
}
