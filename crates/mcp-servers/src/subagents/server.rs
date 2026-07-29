use aether_core::core::AgentDeps;
use aether_core::events::{AgentEvent, SubAgentProgressPayload, TraceContext};
use aether_project::{AetherSettings, AgentCatalog};
use clap::Parser;
use rmcp::{
    RoleServer, ServerHandler,
    handler::server::{
        router::tool::ToolRouter,
        wrapper::{Json, Parameters},
    },
    model::{Implementation, ProgressNotificationParam, ServerCapabilities, ServerInfo},
    service::RequestContext,
    tool, tool_handler, tool_router,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::tools::{AgentExecutor, SpawnSubAgentsInput, SpawnSubAgentsOutput};
use crate::error::ServerInitError;
use crate::workspace_paths::resolve_path;

type ProgressCallback = Box<dyn Fn(&str, &str, &AgentEvent) + Send + Sync>;

/// The name `spawn_subagent` is exposed under, as callers see it on the wire.
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
}

#[doc = include_str!("../docs/subagents_mcp.md")]
#[derive(Clone)]
pub struct SubAgentsMcp {
    catalog: AgentCatalog,
    tool_router: ToolRouter<Self>,
    project_root: PathBuf,
    agent_deps: AgentDeps,
}

impl SubAgentsMcp {
    pub fn from_project_root(project_root: PathBuf) -> Result<Self, ServerInitError> {
        let settings = AetherSettings::load_default(&project_root)
            .map_err(|e| ServerInitError::Other(format!("Failed to load agents: {e}")))?;
        let catalog = if settings.agents.is_empty() {
            AgentCatalog::empty(project_root.clone())
        } else {
            AgentCatalog::from_settings(&project_root, settings)
                .map_err(|e| ServerInitError::Other(format!("Failed to load agents: {e}")))?
        };
        Ok(Self::new(catalog, project_root))
    }

    pub fn new(catalog: AgentCatalog, project_root: PathBuf) -> Self {
        Self { catalog, tool_router: Self::tool_router(), project_root, agent_deps: AgentDeps::default() }
    }

    /// Cross-cutting dependencies (OAuth credentials, observers) passed to
    /// every sub-agent this server spawns.
    pub fn with_agent_deps(mut self, deps: AgentDeps) -> Self {
        self.agent_deps = deps;
        self
    }

    pub fn from_args(args: Vec<String>) -> Result<Self, ServerInitError> {
        let parsed_args = SubAgentsMcpArgs::from_args(args)?;
        let project_root = parsed_args.project_root.unwrap_or_else(|| PathBuf::from("."));
        Self::from_project_root(project_root)
    }

    pub fn from_args_with_default_project_root(
        args: Vec<String>,
        default_root: &Path,
    ) -> Result<Self, ServerInitError> {
        let parsed_args = SubAgentsMcpArgs::from_args(args)?;
        let project_root = parsed_args
            .project_root
            .map_or_else(|| default_root.to_path_buf(), |path| resolve_path(default_root, path));
        Self::from_project_root(project_root)
    }

    fn build_instructions(&self) -> String {
        let mut instructions = include_str!("./instructions.md").to_string();
        let invocable: Vec<_> = self.catalog.agent_invocable().collect();

        if invocable.is_empty() {
            instructions.push_str(
                "\n\n**No sub-agents are currently available.** \
                 The spawn_subagent tool has no registered agents and should not be called.",
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
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for SubAgentsMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("subagents-mcp", "0.1.0"))
            .with_instructions(self.build_instructions())
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
        request: Parameters<SpawnSubAgentsInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<SpawnSubAgentsOutput>, String> {
        let Parameters(args) = request;
        let remote_parent = TraceContext::from_meta(&context.meta);
        let request_instrumentation = self
            .agent_deps
            .observer_factory
            .as_ref()
            .map(|factory| factory.tool_call_request(SPAWN_SUBAGENT_TOOL, remote_parent.as_ref()));

        let child_parent = request_instrumentation.as_ref().and_then(|instrumentation| instrumentation.trace_context());
        let result = self.spawn(args, &context, child_parent).await;
        if let Some(instrumentation) = request_instrumentation {
            instrumentation.finish(result.as_ref().err().map(String::as_str));
        }
        result.map(Json)
    }

    /// Runs the requested tasks, leaving the caller to report the outcome to
    /// whatever is instrumenting the request. Sub-agents trace beneath
    /// `parent_trace_context`, which is this request's own span.
    async fn spawn(
        &self,
        args: SpawnSubAgentsInput,
        context: &RequestContext<RoleServer>,
        parent_trace_context: Option<TraceContext>,
    ) -> Result<SpawnSubAgentsOutput, String> {
        if !args.tasks.is_empty() && self.catalog.agent_invocable().next().is_none() {
            return Err("No agent-invocable sub-agents are registered in this project. \
                 The spawn_subagent tool is not usable — do not call it again."
                .to_string());
        }

        let progress_token = context.meta.get_progress_token();
        let peer = Arc::new(context.peer.clone());
        let message_counter = Arc::new(AtomicU64::new(0));

        let progress_callback: ProgressCallback = {
            let progress_token = progress_token.clone();
            let peer = Arc::clone(&peer);
            let message_counter = Arc::clone(&message_counter);

            Box::new(move |task_id: &str, agent_name: &str, message: &AgentEvent| {
                if let Some(ref token) = progress_token {
                    let counter = message_counter.fetch_add(1, Ordering::Relaxed);
                    let progress_payload = SubAgentProgressPayload {
                        task_id: task_id.to_string(),
                        agent_name: agent_name.to_string(),
                        event: message.clone(),
                    };

                    let peer = Arc::clone(&peer);
                    let token = token.clone();
                    let progress_data_str = serde_json::to_string(&progress_payload).unwrap_or_default();

                    tokio::spawn(async move {
                        let _ = peer
                            .notify_progress(
                                ProgressNotificationParam::new(token, {
                                    #[allow(clippy::cast_precision_loss)]
                                    let progress = counter as f64;
                                    progress
                                })
                                .with_message(progress_data_str),
                            )
                            .await;
                    });
                }
            })
        };

        let deps = self.agent_deps.clone().with_parent_trace_context(parent_trace_context);
        let executor = AgentExecutor::new(self.catalog.clone(), self.project_root.clone(), deps)
            .with_progress_callback(progress_callback);

        Ok(executor.execute_tasks(args.tasks).await)
    }
}
