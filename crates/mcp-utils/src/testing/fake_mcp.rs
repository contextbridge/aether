use rmcp::{
    ErrorData as McpError, Peer, RoleServer, ServerHandler,
    model::{
        CacheScope, CallToolRequestParams, CallToolResponse, CallToolResult, CancelTaskParams, ClientCapabilities,
        ContentBlock, CreateTaskResult, DetailedTask, DiscoverRequestMethod, DiscoverResult, ElicitResult,
        GetPromptRequestParams, GetPromptResponse, GetPromptResult, GetTaskParams, GetTaskResult, Implementation,
        ListPromptsResult, ListToolsResult, PaginatedRequestParams, ProgressNotificationParam, ProgressToken, Prompt,
        PromptMessage, PromptsCapability, ProtocolVersion, ResultType, Role, ServerCapabilities, ServerConfig,
        TaskPayload, Tool, ToolAnnotations, UpdateTaskParams,
    },
    service::RequestContext,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::future::Future;
use std::sync::{Arc, Mutex, Weak};
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;

use super::Gate;

pub fn completed_task_payload(result: CallToolResult) -> TaskPayload {
    let result = serde_json::to_value(result).and_then(serde_json::from_value).expect("a tool result is a JSON object");
    TaskPayload::Completed { result }
}

#[derive(Clone, Default)]
pub struct FakeMcpServer {
    state: FakeMcpState,
    alive: Arc<()>,
}

#[derive(Clone, Default)]
pub struct FakeMcpState {
    inner: Arc<Mutex<FakeMcpStateInner>>,
    changes: Arc<watch::Sender<()>>,
}

#[derive(Clone)]
pub struct CapturedToolCall {
    pub request: CallToolRequestParams,
    pub context_meta: serde_json::Map<String, serde_json::Value>,
    pub at: Instant,
}

#[derive(Clone)]
pub struct CapturedTaskUpdate {
    pub task_id: String,
    pub input_responses: rmcp::model::InputResponses,
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
    gate: Option<Gate>,
    progress: Vec<FakeProgress>,
}

impl FakeMcpServer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn math() -> Self {
        Self::new().with_tool(add_numbers()).with_tool(divide_numbers())
    }

    pub fn with_instructions(self, instructions: impl Into<String>) -> Self {
        self.state.lock().instructions = Some(instructions.into());
        self
    }

    pub fn with_tool(self, tool: FakeTool) -> Self {
        self.state.add_tool(tool);
        self
    }

    pub fn with_prompt(self, name: impl Into<String>, template: impl Into<String>) -> Self {
        let name = name.into();
        self.with_prompt_definition(Prompt::new(name, Some("Fake prompt"), None), template)
    }

    pub fn with_prompt_definition(self, prompt: Prompt, template: impl Into<String>) -> Self {
        self.state.lock().prompts.insert(prompt.name.clone(), (prompt, template.into()));
        self
    }

    pub fn with_prompt_gate(self, gate: Gate) -> Self {
        self.state.lock().prompt_gate = Some(gate);
        self
    }

    pub fn with_tool_list_gate(self, gate: Gate) -> Self {
        self.state.lock().tool_list_gate = Some(gate);
        self
    }

    pub fn with_prompt_elicitation(self, results: mpsc::UnboundedSender<ElicitResult>) -> Self {
        self.state.lock().prompt_elicitations = Some(results);
        self
    }

    pub fn with_task(self, task_id: impl Into<String>, states: impl IntoIterator<Item = DetailedTask>) -> Self {
        self.state.script_task(task_id, states);
        self
    }

    pub fn with_task_get_failures(self, failures: usize) -> Self {
        self.state.lock().task_get_failures = failures;
        self
    }

    pub fn with_task_update_failures(self, failures: usize) -> Self {
        self.state.lock().task_update_failures = failures;
        self
    }

    pub fn with_legacy_protocol(self) -> Self {
        self.state.lock().legacy_protocol = true;
        self
    }

    pub fn state(&self) -> FakeMcpState {
        self.state.clone()
    }

    pub fn alive(&self) -> Weak<()> {
        Arc::downgrade(&self.alive)
    }
}

impl FakeMcpState {
    pub fn calls_for(&self, tool: &str) -> Vec<CapturedToolCall> {
        self.lock().calls.iter().filter(|call| call.request.name.as_ref() == tool).cloned().collect()
    }

    pub fn tool_list_requests(&self) -> usize {
        self.lock().tool_list_requests
    }

    pub fn task_get_ids(&self) -> Vec<String> {
        self.lock().task_get_ids.clone()
    }

    pub fn task_updates(&self) -> Vec<CapturedTaskUpdate> {
        self.lock().task_updates.clone()
    }

    pub fn task_cancel_ids(&self) -> Vec<String> {
        self.lock().task_cancel_ids.clone()
    }

    pub fn client_capabilities(&self) -> Option<ClientCapabilities> {
        self.lock().client_capabilities.clone()
    }

    pub fn script_task(&self, task_id: impl Into<String>, states: impl IntoIterator<Item = DetailedTask>) {
        self.lock().tasks.insert(task_id.into(), states.into_iter().collect());
    }

    pub fn add_tool(&self, tool: FakeTool) {
        self.lock().tools.insert(tool.definition.name.to_string(), tool);
    }

    pub async fn add_tool_and_notify(&self, tool: FakeTool) {
        self.mutate_tools_and_notify(|tools| {
            tools.insert(tool.definition.name.to_string(), tool);
        })
        .await;
    }

    pub async fn clear_tools_and_notify(&self) {
        self.mutate_tools_and_notify(BTreeMap::clear).await;
    }

    pub async fn notify_tools_changed(&self) {
        self.mutate_tools_and_notify(|_| {}).await;
    }

    pub fn fail_next_tool_list(&self) {
        self.lock().tool_list_failures += 1;
    }

    pub async fn wait_until(&self, condition: impl Fn(&Self) -> bool) {
        let mut changes = self.changes.subscribe();
        while !condition(self) {
            changes.changed().await.expect("the state outlives its waiters");
        }
    }

    async fn mutate_tools_and_notify(&self, mutate: impl FnOnce(&mut BTreeMap<String, FakeTool>)) {
        let client = {
            let mut inner = self.lock();
            mutate(&mut inner.tools);
            inner.client.clone()
        };
        if let Some(client) = client {
            let _ = client.notify_tool_list_changed().await;
        }
    }

    fn list_tools(&self, peer: Peer<RoleServer>) -> Result<Vec<Tool>, McpError> {
        self.record(|inner| {
            inner.tool_list_requests += 1;
            if inner.tool_list_failures > 0 {
                inner.tool_list_failures -= 1;
                return Err(McpError::internal_error("scripted tools/list failure", None));
            }
            if inner.client.as_ref().is_none_or(Peer::is_transport_closed) {
                inner.client = Some(peer);
            }
            Ok(inner.tools.values().map(|tool| tool.definition.clone()).collect())
        })
    }

    fn definitions(&self) -> Vec<Tool> {
        self.lock().tools.values().map(|tool| tool.definition.clone()).collect()
    }

    fn response_for(
        &self,
        request: &CallToolRequestParams,
        context_meta: serde_json::Map<String, serde_json::Value>,
    ) -> Option<FakeToolResponse> {
        self.record(|inner| {
            inner.calls.push(CapturedToolCall { request: request.clone(), context_meta, at: Instant::now() });
            inner.tools.get(request.name.as_ref()).and_then(|tool| tool.response_for(request))
        })
    }

    fn task_for(&self, task_id: &str) -> Result<Option<DetailedTask>, ()> {
        self.record(|inner| {
            inner.task_get_ids.push(task_id.to_string());
            if inner.task_get_failures > 0 {
                inner.task_get_failures -= 1;
                return Err(());
            }
            let Some(states) = inner.tasks.get_mut(task_id) else {
                return Ok(None);
            };
            Ok(if states.len() > 1 { states.pop_front() } else { states.front().cloned() })
        })
    }

    fn record_task_update(&self, request: UpdateTaskParams) -> bool {
        self.record(|inner| {
            inner
                .task_updates
                .push(CapturedTaskUpdate { task_id: request.task_id, input_responses: request.input_responses });
            if inner.task_update_failures > 0 {
                inner.task_update_failures -= 1;
                false
            } else {
                true
            }
        })
    }

    fn record_task_cancel(&self, request: CancelTaskParams) {
        self.record(|inner| inner.task_cancel_ids.push(request.task_id));
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, FakeMcpStateInner> {
        self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn record<T>(&self, change: impl FnOnce(&mut FakeMcpStateInner) -> T) -> T {
        let result = change(&mut self.lock());
        self.changes.send_replace(());
        result
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

    pub fn input_schema(mut self, schema: Value) -> Self {
        let Value::Object(schema) = schema else { panic!("a tool input schema must be a JSON object") };
        self.definition.input_schema = Arc::new(schema);
        self
    }

    pub fn annotations(mut self, annotations: ToolAnnotations) -> Self {
        self.definition.annotations = Some(annotations);
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
        Self { response: response.into(), gate: None, progress: Vec::new() }
    }

    pub fn pending() -> Self {
        Self::text("never sent").gated(Gate::new())
    }

    pub fn text(text: impl Into<String>) -> Self {
        Self::new(CallToolResult::success(vec![ContentBlock::text(text.into())]))
    }

    pub fn task(task: CreateTaskResult) -> Self {
        Self::new(CallToolResponse::Task(task))
    }

    pub fn gated(mut self, gate: Gate) -> Self {
        self.gate = Some(gate);
        self
    }

    pub fn progress(mut self, progress: f64, total: Option<f64>) -> Self {
        self.progress.push(FakeProgress { progress, total, message: None });
        self
    }

    pub fn progress_message(mut self, progress: f64, message: impl Into<String>) -> Self {
        self.progress.push(FakeProgress { progress, total: None, message: Some(message.into()) });
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

impl ServerHandler for FakeMcpServer {
    fn discover(
        &self,
        context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<DiscoverResult, McpError>> + Send + '_ {
        if self.state.lock().legacy_protocol {
            return std::future::ready(Err(McpError::method_not_found::<DiscoverRequestMethod>()));
        }
        self.state.lock().client_capabilities = context.meta.client_capabilities();
        std::future::ready(Ok(DiscoverResult::from_server_info(
            ServerHandler::supported_protocol_versions(self).into_owned(),
            ServerHandler::get_info(self),
        )))
    }

    fn get_info(&self) -> ServerConfig {
        let inner = self.state.lock();
        let mut capabilities = ServerCapabilities::builder().enable_tools().enable_tasks().build();
        if !inner.prompts.is_empty() {
            capabilities.prompts = Some(PromptsCapability::default());
        }
        let mut config = ServerConfig::new(capabilities).with_server_info(
            Implementation::new("fake-mcp-server", "0.1.0").with_description("A fake MCP server for testing"),
        );
        if inner.legacy_protocol {
            config = config.with_protocol_version(ProtocolVersion::V_2025_11_25);
        }
        match &inner.instructions {
            Some(instructions) => config.with_instructions(instructions.clone()),
            None => config,
        }
    }

    fn get_task(
        &self,
        request: GetTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<GetTaskResult, McpError>> + Send + '_ {
        let result = match self.state.task_for(&request.task_id) {
            Ok(Some(task)) => Ok(GetTaskResult::new(task)),
            Ok(None) => Err(McpError::invalid_params(format!("unknown task: {}", request.task_id), None)),
            Err(()) => Err(McpError::internal_error("scripted tasks/get failure", None)),
        };
        std::future::ready(result)
    }

    fn update_task(
        &self,
        request: UpdateTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<(), McpError>> + Send + '_ {
        std::future::ready(
            self.state
                .record_task_update(request)
                .then_some(())
                .ok_or_else(|| McpError::internal_error("scripted tasks/update failure", None)),
        )
    }

    fn cancel_task(
        &self,
        request: CancelTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<(), McpError>> + Send + '_ {
        self.state.record_task_cancel(request);
        std::future::ready(Ok(()))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let gate = self.state.lock().tool_list_gate.clone();
        if let Some(gate) = gate {
            gate.pass().await;
        }
        let supports_cache_hints =
            context.protocol_version().is_some_and(|version| version >= ProtocolVersion::V_2026_07_28);
        self.state.list_tools(context.peer).map(|tools| ListToolsResult {
            result_type: Some(ResultType::COMPLETE),
            tools,
            meta: None,
            next_cursor: None,
            ttl_ms: supports_cache_hints.then_some(0),
            cache_scope: supports_cache_hints.then_some(CacheScope::Public),
        })
    }

    fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListPromptsResult, McpError>> + Send + '_ {
        let prompts = self.state.lock().prompts.values().map(|(prompt, _)| prompt.clone()).collect();
        std::future::ready(Ok(ListPromptsResult::with_all_items(prompts)))
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, McpError> {
        let (template, gate, elicitations) = {
            let inner = self.state.lock();
            let template = inner.prompts.get(&request.name).map(|(_, template)| template.clone());
            (template, inner.prompt_gate.clone(), inner.prompt_elicitations.clone())
        };
        if let Some(gate) = gate {
            gate.pass().await;
        }
        if let Some(results) = elicitations {
            let params = serde_json::from_value(json!({
                "mode": "form", "message": "Continue?", "requestedSchema": {"type": "object", "properties": {}}
            }))
            .expect("valid elicitation request");
            let response = context.peer.create_elicitation(params).await.expect("elicitation response");
            let _ = results.send(response);
        }
        template
            .map(|template| {
                let text = request.arguments.unwrap_or_default().iter().fold(template, |text, (name, value)| {
                    text.replace(&format!("{{{name}}}"), value.as_str().unwrap_or_default())
                });
                GetPromptResult::new(vec![PromptMessage::new_text(Role::User, text)]).into()
            })
            .ok_or_else(|| McpError::invalid_params(format!("unknown prompt: {}", request.name), None))
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

        if let Some(gate) = &response.gate {
            gate.pass().await;
        }
        if let Some(token) = context.meta.get_progress_token() {
            let notifications: Vec<_> =
                response.progress.iter().map(|progress| progress.notification(&token)).collect();
            let peer = context.peer.clone();
            let notify = async move {
                for notification in notifications {
                    let _ = peer.notify_progress(notification).await;
                }
            };
            if matches!(response.response, CallToolResponse::Task(_)) {
                tokio::spawn(async move {
                    tokio::task::yield_now().await;
                    notify.await;
                });
            } else {
                notify.await;
            }
        }
        Ok(response.response)
    }
}

type ToolHandler = Arc<dyn Fn(&CallToolRequestParams) -> FakeToolResponse + Send + Sync>;

#[derive(Default)]
struct FakeMcpStateInner {
    tools: BTreeMap<String, FakeTool>,
    instructions: Option<String>,
    prompts: BTreeMap<String, (Prompt, String)>,
    prompt_gate: Option<Gate>,
    tool_list_gate: Option<Gate>,
    prompt_elicitations: Option<mpsc::UnboundedSender<ElicitResult>>,
    calls: Vec<CapturedToolCall>,
    client_capabilities: Option<ClientCapabilities>,
    tasks: HashMap<String, VecDeque<DetailedTask>>,
    task_get_ids: Vec<String>,
    task_updates: Vec<CapturedTaskUpdate>,
    task_cancel_ids: Vec<String>,
    task_get_failures: usize,
    task_update_failures: usize,
    tool_list_requests: usize,
    tool_list_failures: usize,
    legacy_protocol: bool,
    client: Option<Peer<RoleServer>>,
}

#[derive(Clone)]
struct FakeProgress {
    progress: f64,
    total: Option<f64>,
    message: Option<String>,
}

impl FakeProgress {
    fn notification(&self, token: &ProgressToken) -> ProgressNotificationParam {
        let mut notification = ProgressNotificationParam::new(token.clone(), self.progress);
        if let Some(total) = self.total {
            notification = notification.with_total(total);
        }
        if let Some(message) = &self.message {
            notification = notification.with_message(message.clone());
        }
        notification
    }
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

fn int_arg(request: &CallToolRequestParams, name: &str) -> i64 {
    request.arguments.as_ref().and_then(|args| args.get(name)).and_then(serde_json::Value::as_i64).unwrap_or_default()
}
