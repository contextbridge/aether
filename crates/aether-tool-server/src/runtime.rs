use crate::{RemoteConfig, RemoteError};
use mcp_servers::{coding::tools::bash::BashEnvironment, execution_servers};
use mcp_utils::server::tasks::TASK_CONTINUATION_KEY;
use mcp_utils::{
    client::{
        CallToolOptions, McpClientEvent, McpManager, McpSnapshot, McpTransport, RuntimeMcpServer, RuntimeMcpTransport,
        ToolExposure, ToolRoute, call_tool_once, runtime::ConnectionRuntime,
    },
    request_context::{AgentIdentity, GatewayRequestContext, TOOL_CONTEXT_KEY},
    tool_policy::ToolFilter,
};
use rmcp::{
    RoleServer, ServerHandler, ServiceExt,
    model::{
        CallToolRequestParams, CallToolResponse, CancelTaskParams, ClientCapabilities, ClientRequest, ErrorData,
        GetTaskParams, GetTaskResult, Implementation, ListToolsResult, PaginatedRequestParams, ProtocolVersion,
        Request, RequestOptionalParam, ServerCapabilities, ServerConfig, ServerResult, TaskAckResult, Tool,
        UpdateTaskParams,
    },
    service::{DynService, RequestContext},
};
use serde_json::{Map, Value};
use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    sync::{mpsc, watch},
    time::Instant,
};
use uuid::Uuid;

#[derive(Clone)]
pub struct RemoteToolRuntime {
    shared: Arc<Shared>,
    gateway: Option<Arc<crate::gateway::GatewayResources>>,
}

impl RemoteToolRuntime {
    pub async fn new(config: RemoteConfig) -> Result<Self, RemoteError> {
        let socket =
            mcp_utils::tool_gateway::UnixSocketMcpTransport::bind(mcp_utils::tool_gateway::UnixSocketPath::new()?)?;
        let environment = BashEnvironment::new()
            .with_current_exe_dir_on_path()
            .with_var(mcp_utils::tool_gateway::AETHER_MCP_IPC_SOCKET, socket.path().to_string_lossy());
        let executions = Arc::new(crate::execution::ExecutionTasks::default());
        let mut first_party = BTreeMap::new();
        let mut task_managers = Vec::new();
        let mut pending = Vec::new();
        let mut filters = BTreeMap::new();
        for server in config.servers {
            filters.insert(server.name.clone(), server.tools.clone());
            let transport = match server.transport {
                McpTransport::InMemory { spec } => {
                    let backend = match spec.factory.as_str() {
                        "coding" => {
                            let coding = execution_servers::coding(spec.args, &config.root_dir, environment.clone())
                                .map_err(RemoteError::Builtin)?
                                .with_bash_task_scope(executions.clone());
                            task_managers.push(coding.task_manager());
                            coding.into_dyn()
                        }
                        "skills" => execution_servers::skills(spec.args, &config.root_dir)
                            .map_err(RemoteError::Builtin)?
                            .into_dyn(),
                        "review" => execution_servers::review(spec.args, &config.root_dir)
                            .map_err(RemoteError::Builtin)?
                            .into_dyn(),
                        _ => return Err(RemoteError::Invalid("unsupported built-in factory".into())),
                    };
                    first_party.insert(server.name, backend);
                    continue;
                }
                McpTransport::Stdio { command, args, env } => RuntimeMcpTransport::Stdio { command, args, env },
                McpTransport::Http(config) => RuntimeMcpTransport::Http(config),
            };
            pending
                .push(RuntimeMcpServer::new(server.name, transport, ToolExposure::default()).with_tools(server.tools));
        }
        let (events, mut event_rx) = mpsc::channel(64);
        let event_task = tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                if let McpClientEvent::Elicitation(request) = event {
                    let _ = request.response_sender.send(mcp_utils::client::cancel_result());
                }
            }
        });
        let (snapshots, snapshot) = watch::channel(Arc::new(McpSnapshot::default()));
        let mut manager = McpManager::new(events, None)
            .with_root_dir(config.root_dir)
            .with_client_capabilities(ClientCapabilities::builder().enable_tasks().build())
            .with_snapshot_sender(snapshots);
        manager.add_mcps(pending).await.map_err(RemoteError::Connection)?;
        let connections = ConnectionRuntime::spawn(manager, Vec::new());
        let mut runtime = Self {
            gateway: None,
            shared: Arc::new(Shared {
                first_party,
                task_managers,
                cancellation: tokio_util::sync::CancellationToken::new(),
                executions,
                filters,
                snapshot,
                connections: tokio::sync::Mutex::new(Some(connections)),
                event_task,
                continuations: Mutex::new(HashMap::new()),
                tasks: Mutex::new(HashMap::new()),
            }),
        };
        runtime.gateway =
            Some(Arc::new(Box::pin(crate::gateway::GatewayResources::start(runtime.clone(), socket)).await?));
        Ok(runtime)
    }

    pub fn gateway_endpoint(&self) -> Option<&std::path::Path> {
        self.gateway.as_ref().map(|gateway| gateway.endpoint())
    }

    pub(crate) fn cancel_execution(&self) {
        self.shared.cancellation.cancel();
        for manager in &self.shared.task_managers {
            manager.shutdown();
        }
        if let Some(gateway) = &self.gateway {
            gateway.shutdown();
        }
    }

    pub async fn shutdown(&mut self) -> Result<(), RemoteError> {
        self.cancel_execution();
        self.gateway.take();
        if let Some(mut connections) = self.shared.connections.lock().await.take() {
            connections.shutdown().await?;
        }
        self.shared.event_task.abort();
        Ok(())
    }

    async fn catalog(&self, context: &RequestContext<RoleServer>) -> Result<Vec<(String, Tool)>, ErrorData> {
        if self.shared.cancellation.is_cancelled() {
            return Err(ErrorData::internal_error("remote runtime is shut down", None));
        }
        let mut tools = Vec::new();
        for (name, backend) in &self.shared.first_party {
            let request = ClientRequest::ListToolsRequest(RequestOptionalParam::default());
            let response = backend.handle_request(request, context.clone()).await?;
            let ServerResult::ListToolsResult(result) = response else {
                return Err(ErrorData::internal_error("invalid backend tool listing", None));
            };
            tools.extend(result.tools.into_iter().map(|tool| (name.clone(), tool)));
        }
        let snapshot = self.shared.snapshot.borrow().clone();
        for tool in snapshot.catalog().tools().model_visible {
            let (backend, _) = tool.namespaced_name().split_once("__").expect("catalog names are namespaced");
            tools.push((backend.to_string(), tool.mcp_definition().clone()));
        }
        tools.sort_by(|a, b| (&a.0, &a.1.name).cmp(&(&b.0, &b.1.name)));
        Ok(tools)
    }

    pub(crate) async fn input_responder(
        &self,
        policy: &GatewayRequestContext,
        context: &RequestContext<RoleServer>,
    ) -> Result<Arc<dyn mcp_utils::client::InputResponder>, ErrorData> {
        self.authorize("coding__bash", context).await?;
        self.shared.executions.responder(policy)
    }

    pub(crate) async fn authorize(
        &self,
        name: &str,
        context: &RequestContext<RoleServer>,
    ) -> Result<GatewayRequestContext, ErrorData> {
        let policy = request_policy(context)?;
        let permitted = self.catalog(context).await?.into_iter().any(|(backend, tool)| {
            format!("{backend}__{}", tool.name) == name && self.allowed(&policy, &backend, &tool)
        });
        if !permitted {
            return Err(ErrorData::invalid_params("tool is unavailable or not permitted", None));
        }
        Ok(policy)
    }

    fn allowed(&self, policy: &GatewayRequestContext, backend: &str, tool: &Tool) -> bool {
        self.shared.filters.get(backend).is_some_and(|filter| policy.allows(backend, filter, tool))
    }

    async fn dispatch(
        &self,
        backend: &str,
        request: ClientRequest,
        context: RequestContext<RoleServer>,
        tool_name: &str,
    ) -> Result<ServerResult, ErrorData> {
        if let Some(service) = self.shared.first_party.get(backend) {
            return tokio::select! {
                () = self.shared.cancellation.cancelled() => Err(ErrorData::internal_error("remote runtime is shut down", None)),
                result = service.handle_request(request, context) => result,
            };
        }
        let snapshot = self.shared.snapshot.borrow().clone();
        let (client, _) = snapshot
            .resolve(ToolRoute::ModelVisible { namespaced_name: tool_name.to_string() }, Map::new())
            .map_err(|_| ErrorData::internal_error("backend unavailable", None))?;
        match request {
            ClientRequest::CallToolRequest(request) => {
                let mut params = request.params;
                let mut meta = context.meta.clone();
                meta.remove(TOOL_CONTEXT_KEY);
                meta.remove("io.modelcontextprotocol/protocolVersion");
                meta.remove("io.modelcontextprotocol/clientCapabilities");
                meta.remove("io.modelcontextprotocol/clientInfo");
                meta.remove("progressToken");
                if let Some(capabilities) = context.client_capabilities() {
                    meta.set_client_capabilities(capabilities);
                }
                params.meta = Some(meta.clone());
                let token = context.meta.get("progressToken").cloned();
                let peer = context.peer;
                let response = call_tool_once(
                    &client,
                    params,
                    CallToolOptions { timeout: Duration::from_secs(3600), meta: Some(meta), cancel: context.ct },
                    |mut progress| {
                        let peer = peer.clone();
                        let token = token.clone();
                        async move {
                            if let Some(token) = token.and_then(|value| serde_json::from_value(value).ok()) {
                                progress.progress_token = token;
                                let _ = peer.notify_progress(progress).await;
                            }
                        }
                    },
                )
                .await
                .map_err(|error| match error {
                    mcp_utils::client::CallToolError::Call(error) | mcp_utils::client::CallToolError::Send(error) => {
                        backend_error(error)
                    }
                    _ => ErrorData::internal_error("backend tool call failed", None),
                })?;
                Ok(match response {
                    CallToolResponse::Complete(result) => ServerResult::CallToolResult(result),
                    CallToolResponse::InputRequired(result) => ServerResult::InputRequiredResult(result),
                    CallToolResponse::Task(result) => ServerResult::CreateTaskResult(result),
                    _ => return Err(ErrorData::internal_error("unsupported backend response", None)),
                })
            }
            ClientRequest::GetTaskRequest(mut request) => {
                request.params.meta = None;
                client.get_task(request.params).await.map(ServerResult::GetTaskResult).map_err(backend_error)
            }
            ClientRequest::UpdateTaskRequest(mut request) => {
                request.params.meta = None;
                client
                    .update_task(request.params)
                    .await
                    .map(|()| ServerResult::TaskAckResult(TaskAckResult::default()))
                    .map_err(backend_error)
            }
            ClientRequest::CancelTaskRequest(mut request) => {
                request.params.meta = None;
                client
                    .cancel_task(request.params)
                    .await
                    .map(|()| ServerResult::TaskAckResult(TaskAckResult::default()))
                    .map_err(backend_error)
            }
            _ => Err(ErrorData::invalid_params("unsupported backend operation", None)),
        }
    }

    fn register_task(&self, binding: Binding, backend: &str, backend_id: String) -> String {
        let mut tasks = self.shared.tasks.lock().expect("task routes poisoned");
        tasks.retain(|_, route| route.binding.expires > Instant::now());
        if let Some((id, _)) = tasks.iter().find(|(_, route)| {
            route.binding.owner == binding.owner && route.backend == backend && route.backend_id == backend_id
        }) {
            return id.clone();
        }
        let id = Uuid::new_v4().to_string();
        tasks.insert(id.clone(), TaskRoute { binding, backend: backend.to_string(), backend_id });
        id
    }

    async fn task_route(&self, id: &str, context: &RequestContext<RoleServer>) -> Result<TaskRoute, ErrorData> {
        let policy = request_policy(context)?;
        let route =
            self.shared.tasks.lock().expect("task routes poisoned").get(id).cloned().ok_or_else(invalid_handle)?;
        if route.binding.owner != policy.identity || route.binding.expires <= Instant::now() {
            return Err(invalid_handle());
        }
        self.authorize(&route.binding.tool, context).await?;
        Ok(route)
    }
}

impl ServerHandler for RemoteToolRuntime {
    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Owned(vec![ProtocolVersion::V_2026_07_28])
    }

    fn get_info(&self) -> ServerConfig {
        let mut instructions = self
            .shared
            .first_party
            .values()
            .filter_map(|backend| backend.get_info().instructions)
            .collect::<Vec<_>>()
            .join("\n\n");
        instructions.push_str("\n\n# Remote tool composition\n\nIn this remote workspace's Bash, discover deferred tools with `aether-tool-server mcp --help`, inspect a server with `aether-tool-server mcp <server> --help`, and call a tool with `aether-tool-server mcp <server> <tool> --json '{...}'` or JSON stdin. Pipelines and interactive approvals use the same remote execution. Only permitted deferred remote-owned tools are available; harness-local tasks and subagents cannot be called here. Do not use `aether mcp` in remote Bash.");
        ServerConfig::new(ServerCapabilities::builder().enable_tools().enable_tasks().build())
            .with_server_info(Implementation::new("aether-tool-server", env!("CARGO_PKG_VERSION")))
            .with_instructions(instructions)
    }

    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let policy = request_policy(&context)?;
        if request.is_some_and(|request| request.cursor.is_some()) {
            return Err(ErrorData::invalid_params("invalid tool cursor", None));
        }
        let tools = self
            .catalog(&context)
            .await?
            .into_iter()
            .filter_map(|(backend, mut tool)| {
                if !self.allowed(&policy, &backend, &tool) {
                    return None;
                }
                tool.name = format!("{backend}__{}", tool.name).into();
                Some(tool)
            })
            .collect();
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        mut request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let tool_name = request.name.to_string();
        let policy = self.authorize(&tool_name, &context).await?;
        let (backend, local) = tool_name.split_once("__").ok_or_else(invalid_handle)?;
        let arguments = request.arguments.clone();
        if let Some(state) = request.request_state.take() {
            let id = state.as_str();
            let mut continuations = self.shared.continuations.lock().expect("continuations poisoned");
            let continuation = continuations.get(id).ok_or_else(invalid_handle)?;
            if !continuation.binding.matches(&policy, &tool_name, arguments.as_ref()) {
                return Err(invalid_handle());
            }
            request.request_state = continuations.remove(id).expect("validated continuation").state;
        } else if request.input_responses.is_some() {
            return Err(invalid_handle());
        }
        request.name = local.to_string().into();
        let result =
            self.dispatch(backend, ClientRequest::CallToolRequest(Request::new(request)), context, &tool_name).await?;
        let binding = Binding {
            owner: policy.identity,
            tool: tool_name.clone(),
            arguments,
            expires: Instant::now() + Duration::from_secs(3600),
        };
        match result {
            ServerResult::CallToolResult(result) => Ok(CallToolResponse::Complete(result)),
            ServerResult::InputRequiredResult(mut result) => {
                if self.shared.first_party.contains_key(backend)
                    && let Some(task) = result.meta.as_mut().and_then(|meta| meta.get_mut(TASK_CONTINUATION_KEY))
                    && let Some(backend_id) = task.as_str()
                {
                    *task = self.register_task(binding.clone(), backend, backend_id.to_string()).into();
                }
                let id = Uuid::new_v4().to_string();
                let mut continuations = self.shared.continuations.lock().expect("continuations poisoned");
                continuations.retain(|_, value| value.binding.expires > Instant::now());
                continuations.insert(id.clone(), Continuation { binding, state: result.request_state.take() });
                result.request_state = Some(id);
                Ok(CallToolResponse::InputRequired(result))
            }
            ServerResult::CreateTaskResult(mut result) => {
                result.task.task_id = self.register_task(binding, backend, result.task.task_id);
                Ok(CallToolResponse::Task(result))
            }
            _ => Err(ErrorData::internal_error("unsupported backend tool response", None)),
        }
    }

    async fn get_task(
        &self,
        mut request: GetTaskParams,
        context: RequestContext<RoleServer>,
    ) -> Result<GetTaskResult, ErrorData> {
        let route = self.task_route(&request.task_id, &context).await?;
        let public_id = std::mem::replace(&mut request.task_id, route.backend_id);
        let result = self
            .dispatch(
                &route.backend,
                ClientRequest::GetTaskRequest(Request::new(request)),
                context,
                &route.binding.tool,
            )
            .await?;
        let ServerResult::GetTaskResult(mut result) = result else {
            return Err(invalid_handle());
        };
        result.task.task.task_id = public_id;
        Ok(result)
    }

    async fn update_task(
        &self,
        mut request: UpdateTaskParams,
        context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        let route = self.task_route(&request.task_id, &context).await?;
        request.task_id = route.backend_id;
        self.dispatch(
            &route.backend,
            ClientRequest::UpdateTaskRequest(Request::new(request)),
            context,
            &route.binding.tool,
        )
        .await?;
        Ok(())
    }

    async fn cancel_task(
        &self,
        mut request: CancelTaskParams,
        context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        let route = self.task_route(&request.task_id, &context).await?;
        request.task_id = route.backend_id;
        self.dispatch(
            &route.backend,
            ClientRequest::CancelTaskRequest(Request::new(request)),
            context,
            &route.binding.tool,
        )
        .await?;
        Ok(())
    }
}

struct Shared {
    cancellation: tokio_util::sync::CancellationToken,
    task_managers: Vec<rmcp::task_manager::TaskManager>,
    executions: Arc<crate::execution::ExecutionTasks>,
    first_party: BTreeMap<String, Box<dyn DynService<RoleServer>>>,
    filters: BTreeMap<String, ToolFilter>,
    snapshot: watch::Receiver<Arc<McpSnapshot>>,
    connections: tokio::sync::Mutex<Option<ConnectionRuntime>>,
    event_task: tokio::task::JoinHandle<()>,
    continuations: Mutex<HashMap<String, Continuation>>,
    tasks: Mutex<HashMap<String, TaskRoute>>,
}

impl Drop for Shared {
    fn drop(&mut self) {
        self.event_task.abort();
    }
}

#[derive(Clone)]
struct Binding {
    owner: AgentIdentity,
    tool: String,
    arguments: Option<Map<String, Value>>,
    expires: Instant,
}

impl Binding {
    fn matches(&self, policy: &GatewayRequestContext, tool: &str, arguments: Option<&Map<String, Value>>) -> bool {
        self.owner == policy.identity
            && self.tool == tool
            && self.arguments.as_ref() == arguments
            && self.expires > Instant::now()
    }
}

struct Continuation {
    binding: Binding,
    state: Option<String>,
}

#[derive(Clone)]
struct TaskRoute {
    binding: Binding,
    backend: String,
    backend_id: String,
}

fn request_policy(context: &RequestContext<RoleServer>) -> Result<GatewayRequestContext, ErrorData> {
    GatewayRequestContext::from_meta(Some(&context.meta))
        .map_err(|error| ErrorData::invalid_params(error.to_string(), None))
}

fn invalid_handle() -> ErrorData {
    ErrorData::invalid_params("unknown, expired, or unauthorized execution handle", None)
}

fn backend_error(error: rmcp::service::ServiceError) -> ErrorData {
    match error {
        rmcp::service::ServiceError::McpError(error) => error,
        _ => ErrorData::internal_error("backend task operation failed", None),
    }
}
