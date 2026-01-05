use aether::agent::{AgentMessage, substitute_parameters};
use clap::Parser;
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    handler::server::{
        router::tool::ToolRouter,
        wrapper::{Json, Parameters},
    },
    model::{
        GetPromptRequestParam, GetPromptResult, Implementation, ListPromptsResult,
        PaginatedRequestParam, ProgressNotificationParam, PromptMessage, PromptMessageRole,
        ServerCapabilities, ServerInfo,
    },
    service::RequestContext,
    tool, tool_handler, tool_router,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::error;

/// Callback type for reporting agent progress during subagent execution.
type ProgressCallback = Box<dyn Fn(&str, &str, &AgentMessage) + Send + Sync>;

use super::files::{
    AgentFile, PromptFile, SkillInfo as SkillMetadata, SkillsFile, SubAgentInfo,
    load_agent_metadata, load_skill_metadata,
};
use super::tools::{
    AgentExecutor, ListAgentsOutput, ListSkillsOutput, LoadSkillsInput, LoadSkillsOutput, Skill,
    SkillInfo, SpawnSubAgentsInput, SpawnSubAgentsOutput, SubAgentListItem,
};

/// CLI arguments for PluginsMcp server
#[derive(Debug, Clone, Parser)]
pub struct PluginsMcpArgs {
    /// Base directory for plugins (contains 'commands' and 'skills' subdirectories)
    #[arg(long = "dir")]
    pub base_dir: Option<PathBuf>,
}

impl PluginsMcpArgs {
    pub fn from_args(args: Vec<String>) -> Result<Self, String> {
        // Prepend a dummy program name since clap expects it
        let mut full_args = vec!["plugins-mcp".to_string()];
        full_args.extend(args);

        Self::try_parse_from(full_args)
            .map_err(|e| format!("Failed to parse PluginsMcp arguments: {e}"))
    }
}

/// MCP server that dynamically loads slash-commands and skills from markdown files
#[derive(Clone)]
pub struct PluginsMcp {
    commands_dir: PathBuf,
    skills_dir: PathBuf,
    agents_dir: PathBuf,
    skills_info: Vec<SkillMetadata>,
    agents_info: Vec<SubAgentInfo>,
    tool_router: ToolRouter<Self>,
    /// Workspace roots (from MCP protocol or CLI args)
    roots: Arc<RwLock<Vec<PathBuf>>>,
}

impl PluginsMcp {
    /// Create a new PluginsMcp server with the given base directory
    pub fn new(base_dir: PathBuf) -> Self {
        let skills_dir = base_dir.join("skills");
        let agents_dir = base_dir.join("sub-agents");
        let skills_info = load_skill_metadata(&skills_dir);
        let agents_info = load_agent_metadata(&agents_dir);

        Self {
            commands_dir: base_dir.join("commands"),
            skills_dir,
            agents_dir,
            skills_info,
            agents_info,
            tool_router: Self::tool_router(),
            roots: Arc::new(RwLock::new(vec![base_dir])),
        }
    }

    /// Create a new PluginsMcp server from parsed CLI arguments
    /// If no --dir argument is provided, uses the current directory
    pub fn from_args(args: Vec<String>) -> Result<Self, String> {
        let parsed_args = PluginsMcpArgs::from_args(args)?;
        let base_dir = parsed_args.base_dir.unwrap_or_else(|| PathBuf::from("."));
        Ok(Self::new(base_dir))
    }

    /// Set workspace roots.
    ///
    /// Can be used to set roots from MCP protocol or to override CLI arguments.
    pub fn with_roots(mut self, roots: Vec<PathBuf>) -> Self {
        self.roots = Arc::new(RwLock::new(roots));
        self
    }

    /// Get the current base directory (first root).
    fn get_base_dir(&self) -> PathBuf {
        self.roots
            .try_read()
            .ok()
            .and_then(|roots| roots.first().cloned())
            .unwrap_or_else(|| PathBuf::from("."))
    }

    fn build_instructions(&self) -> String {
        let mut instructions = include_str!("./instructions.md").to_string();

        if !self.skills_info.is_empty() {
            instructions.push_str("\n\n## Available Skills\n");
            instructions.push_str("The following skills are available:\n\n");

            for skill in &self.skills_info {
                instructions.push_str(&format!("- **{}**: {}\n", skill.name, skill.description));
            }
        }

        if !self.agents_info.is_empty() {
            instructions.push_str("\n\n## Available Sub-Agents\n");
            instructions.push_str("The following sub-agents are available:\n\n");

            for agent in &self.agents_info {
                instructions.push_str(&format!("- **{}**: {}\n", agent.name, agent.description));
            }
        }

        instructions
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for PluginsMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            server_info: Implementation {
                name: "plugins-mcp".to_string(),
                version: "0.1.0".to_string(),
                title: None,
                icons: None,
                website_url: None,
            },
            instructions: Some(self.build_instructions()),
            capabilities: ServerCapabilities::builder()
                .enable_prompts()
                .enable_tools()
                .build(),
            ..Default::default()
        }
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, McpError> {
        let command_files_with_paths = match PromptFile::from_dir(&self.commands_dir).await {
            Ok(files) => files,
            Err(e) => {
                error!(
                    "Failed to load prompt files from {:?}: {}",
                    self.commands_dir, e
                );

                return Ok(ListPromptsResult {
                    prompts: Vec::new(),
                    next_cursor: None,
                    meta: None,
                });
            }
        };

        let commands = command_files_with_paths
            .iter()
            .filter_map(|(path, file)| {
                let name = path.file_stem()?.to_string_lossy().to_string();
                Some(file.to_prompt(name))
            })
            .collect();

        Ok(ListPromptsResult {
            prompts: commands,
            next_cursor: None,
            meta: None,
        })
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResult, McpError> {
        let prompt_path = self.commands_dir.join(format!("{}.md", request.name));
        let command_file = PromptFile::from_file(&prompt_path).map_err(|e| {
            McpError::invalid_params(format!("Prompt '{}' not found: {}", request.name, e), None)
        })?;

        let arguments = request.arguments.as_ref().map(|json_map| {
            json_map
                .iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect::<HashMap<String, String>>()
        });

        let content = substitute_parameters(&command_file.content, &arguments);
        let messages = vec![PromptMessage::new_text(PromptMessageRole::User, content)];

        Ok(GetPromptResult {
            description: command_file
                .frontmatter
                .as_ref()
                .and_then(|f| f.description.clone()),
            messages,
        })
    }
}

#[tool_router]
impl PluginsMcp {
    #[doc = include_str!("tools/list_skills/description.md")]
    #[tool]
    pub async fn list_skills(&self) -> Result<Json<ListSkillsOutput>, String> {
        let skills_with_dirs =
            match SkillsFile::from_nested_dirs(&self.skills_dir, "SKILL.md").await {
                Ok(skills) => skills,
                Err(e) => {
                    error!(
                        "Failed to load skill files from {:?}: {}",
                        self.skills_dir, e
                    );
                    return Ok(Json(ListSkillsOutput { skills: Vec::new() }));
                }
            };

        let skills = skills_with_dirs
            .iter()
            .filter_map(|(dir, file)| {
                let name = dir.file_name()?.to_string_lossy().to_string();
                let description = file
                    .frontmatter
                    .as_ref()
                    .map(|f| f.description.clone())
                    .unwrap_or_default();
                Some(SkillInfo { name, description })
            })
            .collect();

        Ok(Json(ListSkillsOutput { skills }))
    }

    #[doc = include_str!("tools/get_skills/description.md")]
    #[tool]
    pub async fn get_skills(
        &self,
        request: Parameters<LoadSkillsInput>,
    ) -> Result<Json<LoadSkillsOutput>, String> {
        let Parameters(args) = request;

        let mut skills = Vec::new();
        for skill_name in args.skills {
            let skill_path = self.skills_dir.join(&skill_name).join("SKILL.md");

            if let Ok(skill_file) = SkillsFile::from_file(&skill_path) {
                skills.push(Skill {
                    name: skill_name,
                    content: skill_file.content,
                });
            }
        }

        Ok(Json(LoadSkillsOutput { skills }))
    }

    #[doc = include_str!("tools/list_subagents/description.md")]
    #[tool]
    pub async fn list_subagents(&self) -> Result<Json<ListAgentsOutput>, String> {
        let agents_with_dirs =
            match AgentFile::from_nested_dirs(&self.agents_dir, "AGENTS.md").await {
                Ok(agents) => agents,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    // If agents directory doesn't exist, return empty list
                    Vec::new()
                }
                Err(e) => return Err(format!("Failed to load agents: {e}")),
            };

        let agents = agents_with_dirs
            .iter()
            .filter_map(|(dir, file)| {
                let name = dir.file_name()?.to_string_lossy().to_string();
                let description = file
                    .frontmatter
                    .as_ref()
                    .map(|f| f.description.clone())
                    .unwrap_or_default();
                Some(SubAgentListItem { name, description })
            })
            .collect();

        Ok(Json(ListAgentsOutput { agents }))
    }

    #[doc = include_str!("tools/spawn_subagent/description.md")]
    #[tool]
    pub async fn spawn_subagent(
        &self,
        request: Parameters<SpawnSubAgentsInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<SpawnSubAgentsOutput>, String> {
        let Parameters(args) = request;

        // Set up MCP progress notifications
        let progress_token = context.meta.get_progress_token();
        let peer = Arc::new(context.peer.clone());
        let message_counter = Arc::new(std::sync::atomic::AtomicU64::new(0));

        let progress_callback: ProgressCallback = {
            let progress_token = progress_token.clone();
            let peer = Arc::clone(&peer);
            let message_counter = Arc::clone(&message_counter);

            Box::new(
                move |task_id: &str, agent_name: &str, message: &AgentMessage| {
                    if let Some(ref token) = progress_token {
                        let counter =
                            message_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let progress_data = serde_json::json!({
                            "task_id": task_id,
                            "agent_name": agent_name,
                            "event": serde_json::to_value(message).unwrap_or(serde_json::Value::Null),
                        });

                        let peer = Arc::clone(&peer);
                        let token = token.clone();
                        let progress_data_str = progress_data.to_string();

                        tokio::spawn(async move {
                            let _ = peer
                                .notify_progress(ProgressNotificationParam {
                                    progress_token: token,
                                    progress: counter as f64,
                                    total: None,
                                    message: Some(progress_data_str),
                                })
                                .await;
                        });
                    }
                },
            )
        };

        let executor =
            AgentExecutor::new(self.agents_dir.clone()).with_progress_callback(progress_callback);

        let output = executor.execute_tasks(args.tasks).await;
        Ok(Json(output))
    }
}
