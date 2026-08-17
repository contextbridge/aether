use crate::events::{TaskOutcomeState, TraceContext, task_created_result};
use crate::mcp::tool_bridge::{convert_tool_result, map_task_result_to_outcome};
use crate::mcp::{McpHandle, McpRuntime, ServerFactory, ToolCallStream, mcp};
use futures::{FutureExt, StreamExt};
use mcp_utils::client::{
    CallToolOptions, CancellationToken, InMemoryServerSpec, McpConnectionDetails, McpServer, McpTransport,
    ToolCallEvent, ToolExposure,
};
use mcp_utils::testing::ElicitationScript;
use rmcp::model::{CreateTaskResult, ElicitResult, ProgressNotificationParam};
use rmcp::{RoleServer, ServerHandler, service::DynService};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::watch;

pub use mcp_utils::testing::CapturedElicitation;

const DEFAULT_TOOL_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Default)]
pub struct McpTestBuilder {
    servers: Vec<McpServer>,
    factories: Vec<(String, ServerFactory)>,
    elicitation_responses: Vec<ElicitResult>,
    trace_context: Option<TraceContext>,
    tool_timeout: Duration,
}

fn task_outcome(outcome: crate::events::TaskOutcome) -> TaskOutcome {
    let (status, body) = match outcome.state {
        TaskOutcomeState::Completed { result, .. } => ("completed", result.result),
        TaskOutcomeState::Failed { error } => ("failed", error.error),
        TaskOutcomeState::Cancelled => {
            ("cancelled", "The background task was cancelled and will not produce a result.".into())
        }
    };
    TaskOutcome { task_id: outcome.task_id, status: status.into(), body }
}

pub struct McpTest {
    mcp: McpHandle,
    _runtime: McpRuntime,
    snapshot: McpConnectionDetails,
    elicitations: ElicitationScript,
    deferred_tools: tokio::sync::Mutex<VecDeque<DeferredTool>>,
    cancel_tokens: Mutex<HashMap<String, CancellationToken>>,
    trace_context: Option<TraceContext>,
    tool_timeout: Duration,
    next_call_id: AtomicU64,
    _aether_home: tempfile::TempDir,
}

pub struct TaskOutcome {
    pub task_id: String,
    pub status: String,
    pub body: String,
}

pub struct ToolCallOutcome {
    pub result: Result<llm::ToolCallResult, llm::ToolCallError>,
    pub progress: Vec<ProgressNotificationParam>,
    pub deferred_task: Option<CreateTaskResult>,
}

struct DeferredTool {
    request: llm::ToolCallRequest,
    events: ToolCallStream,
}

impl McpTestBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn server<S>(self, name: impl Into<String>, server: S) -> Self
    where
        S: ServerHandler + Clone + Send + Sync + 'static,
    {
        self.server_with_exposure(name, server, ToolExposure::Direct)
    }

    pub fn proxy_server<T>(self, name: impl Into<String>, server: T) -> Self
    where
        T: ServerHandler + Clone + Send + Sync + 'static,
    {
        self.server_with_exposure(name, server, ToolExposure::proxied_all())
    }

    pub fn server_with_exposure<T>(mut self, name: impl Into<String>, server: T, exposure: ToolExposure) -> Self
    where
        T: ServerHandler + Clone + Send + Sync + 'static,
    {
        let name = name.into();
        let factory_name = format!("test-{}", self.factories.len());
        let factory_server = server;
        let factory: ServerFactory = Box::new(move |_spec, _services| {
            let server = factory_server.clone();
            async move { Box::new(server) as Box<dyn DynService<RoleServer>> }.boxed()
        });
        self.factories.push((factory_name.clone(), factory));
        self.servers.push(McpServer::new(
            name,
            McpTransport::InMemory {
                spec: InMemoryServerSpec { factory: factory_name, args: Vec::new(), input: None },
            },
            exposure,
        ));
        self
    }

    pub fn elicitation_response(mut self, response: ElicitResult) -> Self {
        self.elicitation_responses.push(response);
        self
    }

    pub fn trace_context(mut self, trace_context: TraceContext) -> Self {
        self.trace_context = Some(trace_context);
        self
    }

    pub fn tool_timeout(mut self, timeout: Duration) -> Self {
        self.tool_timeout = timeout;
        self
    }

    pub async fn build(self) -> McpTest {
        let aether_home = tempfile::tempdir().expect("temp aether home is created");
        let builder = self.factories.into_iter().fold(
            mcp("/workspace").with_aether_home(aether_home.path()).with_servers(self.servers),
            |builder, (name, factory)| builder.register_in_memory_server(name, factory),
        );
        let mut spawn = builder.spawn().await.expect("MCP test manager spawns");
        let snapshot = spawn.block_until_ready().await.expect("MCP test manager becomes ready");
        let (runtime, event_rx) = spawn.split();

        McpTest {
            mcp: runtime.handle().clone(),
            _runtime: runtime,
            snapshot,
            elicitations: ElicitationScript::spawn(event_rx, self.elicitation_responses),
            deferred_tools: tokio::sync::Mutex::new(VecDeque::new()),
            cancel_tokens: Mutex::new(HashMap::new()),
            trace_context: self.trace_context,
            tool_timeout: if self.tool_timeout.is_zero() { DEFAULT_TOOL_TIMEOUT } else { self.tool_timeout },
            next_call_id: AtomicU64::new(1),
            _aether_home: aether_home,
        }
    }
}

impl McpTest {
    pub async fn call(&self, server: &str, tool: &str, arguments: Value) -> ToolCallOutcome {
        let id = self.next_call_id.fetch_add(1, Ordering::Relaxed);
        let request = llm::ToolCallRequest {
            id: format!("mcp-test-{id}"),
            name: format!("{server}__{tool}"),
            arguments: arguments.to_string(),
        };
        let request_for_outcome = request.clone();
        let cancel = CancellationToken::new();
        self.cancel_tokens.lock().expect("cancel token lock").insert(request.id.clone(), cancel.clone());
        let options = CallToolOptions {
            timeout: self.tool_timeout,
            meta: self.trace_context.as_ref().map(TraceContext::to_meta),
            cancel,
        };
        let mut events = self.mcp.call_model_visible(request.name, &request.arguments, options);

        let mut progress = Vec::new();
        while let Some(event) = events.next().await {
            match event {
                ToolCallEvent::Progress(event) => progress.push(event),
                ToolCallEvent::TaskCreated(task) => {
                    self.deferred_tools
                        .lock()
                        .await
                        .push_back(DeferredTool { request: request_for_outcome.clone(), events });
                    return ToolCallOutcome {
                        result: Ok(task_created_result(&request_for_outcome, &task.task.task_id)),
                        progress,
                        deferred_task: Some(task),
                    };
                }
                ToolCallEvent::Complete(outcome) => {
                    let result = convert_tool_result(&request_for_outcome, outcome).map(|(result, _)| result);
                    return ToolCallOutcome { result, progress, deferred_task: None };
                }
                ToolCallEvent::TaskStatus(_) | ToolCallEvent::TaskComplete { .. } | ToolCallEvent::Cancelled { .. } => {
                    panic!("MCP task lifecycle event arrived before deferral")
                }
            }
        }
        panic!("MCP test tool event stream ended before completion");
    }

    pub fn cancel_tool(&self, tool_id: &str) {
        let tokens = self.cancel_tokens.lock().expect("cancel token lock");
        tokens.get(tool_id).expect("cancel_tool targets a tool started with call()").cancel();
    }

    pub async fn next_tool_event(&self) -> Option<ToolCallEvent> {
        self.next_deferred_event().await.map(|(_, event)| event)
    }

    pub async fn next_task_outcome(&self) -> Option<TaskOutcome> {
        while let Some((request, event)) = self.next_deferred_event().await {
            match event {
                ToolCallEvent::TaskComplete { task, result } => {
                    return Some(task_outcome(map_task_result_to_outcome(request, task, result)));
                }
                ToolCallEvent::Cancelled { task_id } => {
                    return Some(task_outcome(crate::events::TaskOutcome {
                        request,
                        task_id: task_id.unwrap_or_else(|| "pending".to_string()),
                        state: TaskOutcomeState::Cancelled,
                    }));
                }
                ToolCallEvent::Progress(_)
                | ToolCallEvent::TaskCreated(_)
                | ToolCallEvent::TaskStatus(_)
                | ToolCallEvent::Complete(_) => {}
            }
        }
        None
    }

    async fn next_deferred_event(&self) -> Option<(llm::ToolCallRequest, ToolCallEvent)> {
        let mut deferred = self.deferred_tools.lock().await;
        loop {
            let front = deferred.front_mut()?;
            match front.events.next().await {
                Some(event) => return Some((front.request.clone(), event)),
                None => {
                    deferred.pop_front();
                }
            }
        }
    }

    pub fn snapshot(&self) -> &McpConnectionDetails {
        &self.snapshot
    }

    pub fn subscribe(&self) -> watch::Receiver<McpConnectionDetails> {
        self.mcp.subscribe()
    }

    pub fn elicitations(&self) -> Vec<CapturedElicitation> {
        self.elicitations.captured()
    }
}
