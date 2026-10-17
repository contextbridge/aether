use crate::server_config;
use aether_core::core::AgentDeps;
use aether_core::events::{McpRequestInstrumentation, TraceContext};
use aether_project::{AetherSettings, AgentCatalog, SettingsError};
use clap::Parser;
use rmcp::model::{CallToolResult, Task};
use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResponse, CreateTaskResult, ServerCapabilities, ServerConfig},
    service::RequestContext,
    task_manager::TaskExit,
    tool, tool_handler, tool_router,
};
use std::path::{Path, PathBuf};

use super::progress::SubAgentProgressSink;
use super::tools::{AgentExecutor, SpawnSubAgentsInput, SpawnSubAgentsOutput};
use crate::Displayed;
use crate::background_tasks::{BackgroundTasks, delegate_background_tasks};
use crate::request_context::require_tasks;
use crate::workspace_paths::resolve_root;

const SPAWN_SUBAGENT_TOOL: &str = "spawn_subagent";

#[derive(Debug, Clone, Parser)]
#[command(name = "subagents", no_binary_name = true)]
pub struct SubAgentsMcpArgs {
    /// Project root containing optional .aether/settings.json
    #[arg(long = "project-root", alias = "dir")]
    pub project_root: Option<PathBuf>,
}

#[doc = include_str!("../docs/subagents_mcp.md")]
pub struct SubAgentsMcp {
    tool_router: ToolRouter<Self>,
    tasks: BackgroundTasks,
    project_root: PathBuf,
    agent_deps: AgentDeps,
}

impl SubAgentsMcp {
    pub fn new(project_root: PathBuf, agent_deps: AgentDeps) -> Self {
        Self { tool_router: Self::tool_router(), tasks: BackgroundTasks::new(), project_root, agent_deps }
    }

    pub fn from_args(args: SubAgentsMcpArgs, base_dir: &Path, agent_deps: AgentDeps) -> Self {
        Self::new(resolve_root(base_dir, args.project_root), agent_deps)
    }

    pub fn standalone(args: SubAgentsMcpArgs, base_dir: &Path) -> Result<Self, SettingsError> {
        let project_root = resolve_root(base_dir, args.project_root);
        let settings = AetherSettings::load_default(&project_root)?;
        let catalog = AgentCatalog::from_settings_or_empty(&project_root, settings)?;
        let registry = catalog.registry().clone();
        Ok(Self::new(project_root, AgentDeps::default().with_agent_registry(registry)))
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
        let executor = AgentExecutor::new(self.project_root.clone(), deps);
        match context.meta.get_progress_token() {
            Some(token) => executor.with_progress(SubAgentProgressSink::new(context.peer.clone(), token)),
            None => executor,
        }
    }

    fn validate(&self, args: &SpawnSubAgentsInput, context: &RequestContext<RoleServer>) -> Result<(), ErrorData> {
        if !args.tasks.is_empty() && self.agent_deps.agent_registry.agent_invocable().next().is_none() {
            return Err(ErrorData::invalid_params(
                "No agent-invocable sub-agents are registered in this project. The spawn_subagent tool is not usable — do not call it again.",
                None,
            ));
        }
        if args.run_in_background {
            require_tasks(context)?;
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

        self.tasks.spawn(status, async move {
            let result = output_result(executor.execute_tasks(args.tasks).await);
            instrumentation.finish(result.as_ref().err().map(ToString::to_string).as_deref());
            result.map_err(TaskExit::Error)
        })
    }
}

#[allow(clippy::unused_async_trait_impl)]
#[tool_handler(router = self.tool_router)]
impl ServerHandler for SubAgentsMcp {
    fn get_info(&self) -> ServerConfig {
        server_config(
            "subagents-mcp",
            ServerCapabilities::builder().enable_tools().enable_tasks().build(),
            self.build_instructions(),
        )
    }

    delegate_background_tasks!(tasks);
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

fn output_result(output: Displayed<SpawnSubAgentsOutput>) -> Result<CallToolResult, ErrorData> {
    output.into_result().map_err(|error| ErrorData::internal_error(error.to_string(), None))
}
