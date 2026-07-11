use aether_auth::OAuthCredentialStorage;
use aether_core::events::AgentEvent;
use aether_core::events::SubAgentProgressPayload;
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
    oauth_credential_store: Option<Arc<dyn OAuthCredentialStorage>>,
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
        Self { catalog, tool_router: Self::tool_router(), project_root, oauth_credential_store: None }
    }

    pub fn with_oauth_credential_store(mut self, store: Arc<dyn OAuthCredentialStorage>) -> Self {
        self.oauth_credential_store = Some(store);
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
                            .notify_progress(ProgressNotificationParam {
                                progress_token: token,
                                #[allow(clippy::cast_precision_loss)]
                                progress: counter as f64,
                                total: None,
                                message: Some(progress_data_str),
                            })
                            .await;
                    });
                }
            })
        };

        let executor =
            AgentExecutor::new(self.catalog.clone(), self.project_root.clone(), self.oauth_credential_store.clone())
                .with_progress_callback(progress_callback);

        let output = executor.execute_tasks(args.tasks).await;
        Ok(Json(output))
    }
}
