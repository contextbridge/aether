use crate::events::{TaskOutcomeState, TraceContext, task_created_result};
use crate::mcp::tool_bridge::{call_tool, convert_tool_result, map_task_result_to_outcome};
use crate::mcp::{McpRuntime, mcp};
use futures::StreamExt;
use mcp_utils::client::{CancellationToken, ToolCall, ToolCallError, ToolCallEvent, ToolCallOptions, Transport};
use mcp_utils::gateway::{McpCatalog, ServerSpec, ToolExposure, ToolFilter, namespaced};
use mcp_utils::server::McpServer;
use mcp_utils::testing::{ElicitationScript, ElicitationScriptBuilder};
use rmcp::ServerHandler;
use rmcp::model::{CreateTaskResult, ElicitResult, ProgressNotificationParam};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use utils::temp_dir::TempDir;

pub use mcp_utils::testing::CapturedElicitation;

const DEFAULT_TOOL_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Default)]
pub struct McpTestBuilder {
    servers: Vec<ServerSpec>,
    elicitations: ElicitationScriptBuilder,
    trace_context: Option<TraceContext>,
    tool_timeout: Duration,
    tool_filter: ToolFilter,
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
    runtime: McpRuntime,
    catalog: Arc<McpCatalog>,
    elicitations: ElicitationScript,
    deferred_tools: tokio::sync::Mutex<VecDeque<DeferredTool>>,
    cancel_tokens: Mutex<HashMap<String, CancellationToken>>,
    trace_context: Option<TraceContext>,
    tool_timeout: Duration,
    next_call_id: AtomicU64,
    spill_dir: TempDir,
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
    events: ToolCall,
}

impl McpTestBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn server(self, name: impl Into<String>, server: impl ServerHandler) -> Self {
        self.server_with_exposure(name, server, ToolExposure::ModelVisible)
    }

    pub fn deferred_server(self, name: impl Into<String>, server: impl ServerHandler) -> Self {
        self.server_with_exposure(name, server, ToolExposure::deferred_all())
    }

    pub fn server_with_exposure(
        mut self,
        name: impl Into<String>,
        server: impl ServerHandler,
        exposure: ToolExposure,
    ) -> Self {
        let transport = Transport::InProcess(McpServer::new(server));
        self.servers.push(ServerSpec { name: name.into(), transport, exposure });
        self
    }

    pub fn elicitation_response(mut self, response: ElicitResult) -> Self {
        self.elicitations = self.elicitations.response(response);
        self
    }

    pub fn on_url_elicitation<T, Fut>(mut self, handler: T) -> Self
    where
        T: Fn(String, String) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.elicitations = self.elicitations.on_url(handler);
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

    /// Filter which tools the servers expose and the gateway serves.
    pub fn tool_filter(mut self, filter: ToolFilter) -> Self {
        self.tool_filter = filter;
        self
    }

    pub async fn build(self) -> McpTest {
        let (sink, events) = mpsc::channel(32);
        let builder =
            mcp("/workspace").with_servers(self.servers).with_tool_filter(self.tool_filter).with_elicitations(sink);
        let runtime = builder.spawn().expect("MCP test gateway spawns");
        let catalog = runtime.gateway().ready().await;

        McpTest {
            runtime,
            catalog,
            elicitations: self.elicitations.spawn(events),
            deferred_tools: tokio::sync::Mutex::new(VecDeque::new()),
            cancel_tokens: Mutex::new(HashMap::new()),
            trace_context: self.trace_context,
            tool_timeout: if self.tool_timeout.is_zero() { DEFAULT_TOOL_TIMEOUT } else { self.tool_timeout },
            next_call_id: AtomicU64::new(1),
            spill_dir: TempDir::new(),
        }
    }
}

impl McpTest {
    pub async fn call(&self, server: &str, tool: &str, arguments: Value) -> ToolCallOutcome {
        let id = self.next_call_id.fetch_add(1, Ordering::Relaxed);
        let request = llm::ToolCallRequest {
            id: format!("mcp-test-{id}"),
            name: namespaced(server, tool),
            arguments: arguments.to_string(),
        };
        let cancel = CancellationToken::new();
        self.cancel_tokens.lock().expect("cancel token lock").insert(request.id.clone(), cancel.clone());
        let options = ToolCallOptions {
            timeout: Some(self.tool_timeout),
            meta: self.trace_context.as_ref().map(TraceContext::to_meta),
            cancel,
        };
        let mut events = call_tool(Some(self.runtime.gateway()), &request, options);

        let mut progress = Vec::new();
        while let Some(event) = events.next().await {
            match event {
                ToolCallEvent::Progress(event) => progress.push(event),
                ToolCallEvent::TaskCreated(task) => {
                    self.deferred_tools.lock().await.push_back(DeferredTool { request: request.clone(), events });
                    return ToolCallOutcome {
                        result: Ok(task_created_result(&request, &task.task.task_id)),
                        progress,
                        deferred_task: Some(task),
                    };
                }
                ToolCallEvent::Done { result, .. } => {
                    let result = convert_tool_result(&request, result, &self.spill_dir).map(|(result, _)| result);
                    return ToolCallOutcome { result, progress, deferred_task: None };
                }
                ToolCallEvent::TaskStatus(_) => panic!("MCP task lifecycle event arrived before deferral"),
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
                ToolCallEvent::Done { task, result } => {
                    let task_id = task.map_or_else(|| "pending".to_string(), |task| task.task_id);
                    let outcome = match result {
                        Err(ToolCallError::Cancelled) => {
                            crate::events::TaskOutcome { request, task_id, state: TaskOutcomeState::Cancelled }
                        }
                        result => map_task_result_to_outcome(request, task_id, result, &self.spill_dir),
                    };
                    return Some(task_outcome(outcome));
                }
                ToolCallEvent::Progress(_) | ToolCallEvent::TaskCreated(_) | ToolCallEvent::TaskStatus(_) => {}
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

    pub fn catalog(&self) -> &Arc<McpCatalog> {
        &self.catalog
    }

    pub fn deferred_tools_socket(&self) -> Option<&Path> {
        self.runtime.deferred_tools_socket()
    }

    pub fn elicitations(&self) -> Vec<CapturedElicitation> {
        self.elicitations.captured()
    }
}
