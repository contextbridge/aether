use aether_core::core::AgentDeps;
use aether_core::events::{AgentEvent, McpRequestInstrumentation, SubAgentProgressPayload, TraceContext};
use aether_project::{AetherSettings, AgentCatalog};
use clap::Parser;
use mcp_utils::server::tasks::{BACKGROUND_TASK_TTL_MS, require_tasks_capability};
use rmcp::model::{CallToolResult, Task};
use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{
        CallToolResponse, CancelTaskParams, CreateTaskResult, GetTaskParams, GetTaskResult, Implementation,
        ProgressNotificationParam, ServerCapabilities, ServerInfo, UpdateTaskParams,
    },
    service::RequestContext,
    task_manager::{TaskContext, TaskExit, TaskManager, TaskOptions},
    tool, tool_handler, tool_router,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::tools::{AgentExecutor, SpawnSubAgentsInput, SpawnSubAgentsOutput};
use crate::error::ServerInitError;
use crate::workspace_paths::resolve_path;

type ProgressCallback = Box<dyn Fn(&str, &str, &AgentEvent) + Send + Sync>;

const SPAWN_SUBAGENT_TOOL: &str = "spawn_subagent";

#[derive(Debug, Clone, Parser)]
pub struct SubAgentsMcpArgs {
    /// Project root containing optional .aether/settings.json
    #[arg(long = "project-root", alias = "dir")]
    pub project_root: Option<PathBuf>,
}

impl SubAgentsMcpArgs {
    pub fn from_args(args: Vec<String>) -> Result<Self, ServerInitError> {
        let mut full_args = vec!["subagents-mcp".to_string()];
        full_args.extend(args);
        Self::try_parse_from(full_args).map_err(ServerInitError::InvalidArgs)
    }

    fn resolve_root(self, default_root: &Path) -> PathBuf {
        self.project_root.map_or_else(|| default_root.to_path_buf(), |path| resolve_path(default_root, path))
    }
}

#[doc = include_str!("../docs/subagents_mcp.md")]
pub struct SubAgentsMcp {
    tool_router: ToolRouter<Self>,
    task_manager: TaskManager,
    project_root: PathBuf,
    agent_deps: AgentDeps,
}

impl SubAgentsMcp {
    pub fn embedded(project_root: PathBuf, agent_deps: AgentDeps) -> Self {
        Self { tool_router: Self::tool_router(), task_manager: TaskManager::new(), project_root, agent_deps }
    }

    pub fn embedded_from_args(
        args: Vec<String>,
        base_dir: &Path,
        agent_deps: AgentDeps,
    ) -> Result<Self, ServerInitError> {
        Ok(Self::embedded(SubAgentsMcpArgs::from_args(args)?.resolve_root(base_dir), agent_deps))
    }

    pub fn standalone(project_root: PathBuf) -> Result<Self, ServerInitError> {
        let settings = AetherSettings::load_default(&project_root)
            .map_err(|e| ServerInitError::Other(format!("Failed to load agents: {e}")))?;
        let catalog = AgentCatalog::from_settings_or_empty(&project_root, settings)
            .map_err(|e| ServerInitError::Other(format!("Failed to load agents: {e}")))?;
        let registry = catalog.registry().clone();
        Ok(Self::embedded(project_root, AgentDeps::default().with_agent_registry(registry)))
    }

    pub fn standalone_from_args(args: Vec<String>) -> Result<Self, ServerInitError> {
        Self::standalone(SubAgentsMcpArgs::from_args(args)?.resolve_root(Path::new(".")))
    }

    fn build_instructions(&self) -> String {
        let mut instructions = include_str!("./instructions.md").to_string();
        let mut invocable = self.agent_deps.agent_registry.agent_invocable().peekable();

        if invocable.peek().is_none() {
            instructions.push_str(
                "\n\n**No sub-agents are currently available.** The spawn_subagent tool has no registered agents and should not be called.",
            );
        } else {
            instructions.push_str("\n\n## Available Sub-Agents\n");
            instructions.push_str("The following sub-agents are available:\n\n");
            for agent in invocable {
                use std::fmt::Write as _;
                let _ = writeln!(instructions, "- **{}**: {}", agent.name, agent.description);
            }
        }
        instructions
    }

    fn instrumentation(&self, context: &RequestContext<RoleServer>) -> Option<Box<dyn McpRequestInstrumentation>> {
        let trace_context = TraceContext::from_meta(&context.meta);
        self.agent_deps
            .observer_factory
            .as_ref()
            .map(|factory| factory.tool_call_request(SPAWN_SUBAGENT_TOOL, trace_context.as_ref()))
    }

    fn executor(&self, context: &RequestContext<RoleServer>, child_parent: Option<TraceContext>) -> AgentExecutor {
        let deps = self.agent_deps.clone().with_parent_trace_context(child_parent);
        let callback = progress_callback(context.meta.get_progress_token(), context.peer.clone());
        AgentExecutor::new(self.project_root.clone(), deps).with_progress_callback(callback)
    }

    fn validate(&self, args: &SpawnSubAgentsInput, context: &RequestContext<RoleServer>) -> Result<(), ErrorData> {
        if !args.tasks.is_empty() && self.agent_deps.agent_registry.agent_invocable().next().is_none() {
            return Err(ErrorData::invalid_params(
                "No agent-invocable sub-agents are registered in this project. The spawn_subagent tool is not usable — do not call it again.",
                None,
            ));
        }
        if args.run_in_background {
            require_tasks_capability(context)?;
        }
        Ok(())
    }

    async fn spawn_subagents_in_foreground(
        &self,
        args: SpawnSubAgentsInput,
        context: &RequestContext<RoleServer>,
        child_parent: Option<TraceContext>,
    ) -> Result<CallToolResponse, ErrorData> {
        let output = self.executor(context, child_parent).execute_tasks(args.tasks).await;
        output_result(output).map(CallToolResponse::Complete)
    }

    fn spawn_subagents_in_background(
        &self,
        args: SpawnSubAgentsInput,
        context: &RequestContext<RoleServer>,
        instrumentation: InstrumentationGuard,
    ) -> Task {
        let status = match args.tasks.as_slice() {
            [] => "No sub-agents requested".to_string(),
            [task] => format!("Running {}", task.agent_name),
            tasks => format!("Running {} sub-agents", tasks.len()),
        };
        let executor = self.executor(context, instrumentation.trace_context());
        let options = TaskOptions::new().with_ttl_ms(BACKGROUND_TASK_TTL_MS).with_status_message(status);

        self.task_manager.spawn(options, move |task_context: TaskContext| {
            Box::pin(async move {
                tokio::select! {
                    () = task_context.cancelled() => Err(TaskExit::Cancelled),
                    output = executor.execute_tasks(args.tasks) => {
                        let result = output_result(output);
                        instrumentation.finish(result.as_ref().err().map(ToString::to_string).as_deref());
                        result.map_err(TaskExit::Error)
                    }
                }
            })
        })
    }
}

#[allow(clippy::unused_async_trait_impl)]
#[tool_handler(router = self.tool_router)]
impl ServerHandler for SubAgentsMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().enable_tasks().build())
            .with_server_info(Implementation::new("subagents-mcp", "0.1.0"))
            .with_instructions(self.build_instructions())
    }

    fn get_task(
        &self,
        request: GetTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<GetTaskResult, ErrorData>> + Send + '_ {
        std::future::ready(self.task_manager.get_task(&request.task_id).map(GetTaskResult::new))
    }

    fn update_task(
        &self,
        request: UpdateTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<(), ErrorData>> + Send + '_ {
        std::future::ready(self.task_manager.update_task(&request.task_id, request.input_responses))
    }

    fn cancel_task(
        &self,
        request: CancelTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<(), ErrorData>> + Send + '_ {
        std::future::ready(self.task_manager.cancel_task(&request.task_id))
    }
}

#[tool_router]
impl SubAgentsMcp {
    #[doc = include_str!("tools/spawn_subagent/description.md")]
    #[tool(annotations(
        read_only_hint = false,
        destructive_hint = true,
        idempotent_hint = false,
        open_world_hint = true
    ))]
    pub async fn spawn_subagent(
        &self,
        Parameters(args): Parameters<SpawnSubAgentsInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let instrumentation = InstrumentationGuard(self.instrumentation(&context));

        if let Err(error) = self.validate(&args, &context) {
            instrumentation.finish(Some(&error.to_string()));
            return Err(error);
        }

        if args.run_in_background {
            let task = self.spawn_subagents_in_background(args, &context, instrumentation);
            Ok(CallToolResponse::Task(CreateTaskResult::new(task)))
        } else {
            let child_parent = instrumentation.trace_context();
            let result = self.spawn_subagents_in_foreground(args, &context, child_parent).await;
            instrumentation.finish(result.as_ref().err().map(ToString::to_string).as_deref());
            result
        }
    }
}

impl Drop for SubAgentsMcp {
    fn drop(&mut self) {
        self.task_manager.shutdown();
    }
}

struct InstrumentationGuard(Option<Box<dyn McpRequestInstrumentation>>);

impl InstrumentationGuard {
    fn trace_context(&self) -> Option<TraceContext> {
        self.0.as_deref().and_then(McpRequestInstrumentation::trace_context)
    }

    fn finish(mut self, error: Option<&str>) {
        if let Some(instrumentation) = self.0.take() {
            instrumentation.finish(error);
        }
    }
}

impl Drop for InstrumentationGuard {
    fn drop(&mut self) {
        if let Some(instrumentation) = self.0.take() {
            instrumentation.finish(Some("Sub-agent execution cancelled"));
        }
    }
}

fn output_result(output: SpawnSubAgentsOutput) -> Result<CallToolResult, ErrorData> {
    serde_json::to_value(output)
        .map(CallToolResult::structured)
        .map_err(|error| ErrorData::internal_error(error.to_string(), None))
}

fn progress_callback(
    progress_token: Option<rmcp::model::ProgressToken>,
    peer: rmcp::Peer<RoleServer>,
) -> ProgressCallback {
    let Some(token) = progress_token else {
        return Box::new(|_, _, _| {});
    };
    let message_counter = AtomicU64::new(0);
    Box::new(move |task_id: &str, agent_name: &str, message: &AgentEvent| {
        let counter = message_counter.fetch_add(1, Ordering::Relaxed);
        let payload = SubAgentProgressPayload {
            task_id: task_id.to_string(),
            agent_name: agent_name.to_string(),
            event: message.clone(),
        };
        let peer = peer.clone();
        let token = token.clone();
        let message = serde_json::to_string(&payload).unwrap_or_default();
        tokio::spawn(async move {
            #[allow(clippy::cast_precision_loss)]
            let progress = counter as f64;
            let _ = peer.notify_progress(ProgressNotificationParam::new(token, progress).with_message(message)).await;
        });
    })
}
