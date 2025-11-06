use aether::{
    agent::{AgentHandle, AgentMessage, UserMessage, agent, substitute_parameters},
    llm::{StreamingModelProvider, ToolDefinition, parser::ModelProviderParser},
    mcp::{mcp, run_mcp_task::McpCommand},
};
use clap::Parser;
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler, ServiceExt,
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
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::files::{AgentFile, PromptFile, SkillsFile};
use crate::coding::CodingMcp;

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

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LoadSkillsInput {
    /// Array of skill names to load, e.g. "kungfu"
    pub skills: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
    pub name: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LoadSkillsOutput {
    pub skills: Vec<Skill>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListSkillsOutput {
    pub skills: Vec<SkillInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SubAgentListItem {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListAgentsOutput {
    pub agents: Vec<SubAgentListItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SpawnSubAgentInput {
    /// Name of the agent to spawn (must exist in sub-agents directory)
    pub agent_name: String,
    /// Task for the agent to perform
    pub prompt: String,
    /// Optional model override in the format of "provider:model" (e.g., "anthropic:claude-3.5-sonnet")
    /// If not provided, uses model from agent's AGENT.md frontmatter
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SpawnSubAgentOutput {
    /// The final output from the sub-agent after completing its task.
    /// Progress updates are sent via MCP progress notifications during execution.
    pub output: String,
}

/// MCP server that dynamically loads slash-commands and skills from markdown files
#[derive(Clone)]
pub struct PluginsMcp {
    commands_dir: PathBuf,
    skills_dir: PathBuf,
    agents_dir: PathBuf,
    tool_router: ToolRouter<Self>,
}

impl PluginsMcp {
    /// Create a new PluginsMcp server with the given base directory
    pub fn new(base_dir: PathBuf) -> Self {
        Self {
            commands_dir: base_dir.join("commands"),
            skills_dir: base_dir.join("skills"),
            agents_dir: base_dir.join("sub-agents"),
            tool_router: Self::tool_router(),
        }
    }

    /// Create a new PluginsMcp server from parsed CLI arguments
    /// If no --dir argument is provided, uses the current directory
    pub fn from_args(args: Vec<String>) -> Result<Self, String> {
        let parsed_args = PluginsMcpArgs::from_args(args)?;
        let base_dir = parsed_args.base_dir.unwrap_or_else(|| PathBuf::from("."));
        Ok(Self::new(base_dir))
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
            instructions: None,
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
        let command_files_with_paths = PromptFile::from_dir(&self.commands_dir)
            .await
            .map_err(|e| McpError::internal_error(format!("Failed to load commands: {e}"), None))?;

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
    #[tool(
        description = "List all available skills with their names and descriptions.

Returns an array of skills, each with:
- name: identifier for the skill
- description: summary of what the skill does

Use this to discover available skills before loading their full content with get_skills."
    )]
    pub async fn list_skills(&self) -> Result<Json<ListSkillsOutput>, String> {
        let skills_with_dirs = SkillsFile::from_nested_dirs(&self.skills_dir, "SKILL.md")
            .await
            .map_err(|e| format!("Failed to load skills: {e}"))?;

        let skills = skills_with_dirs
            .iter()
            .filter_map(|(dir, file)| {
                let name = dir.file_name()?.to_string_lossy().to_string();
                let description = file
                    .frontmatter
                    .as_ref()
                    .and_then(|f| f.description.clone())
                    .unwrap_or_default();
                Some(SkillInfo { name, description })
            })
            .collect();

        Ok(Json(ListSkillsOutput { skills }))
    }

    #[tool(description = "Load the full content of one or more skills by name.

Takes an array of skill names and loads them into your context.
Skills that don't exist are silently skipped.

Use list_skills first to discover available skills.")]
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

    #[tool(
        description = "List all available sub-agents with their names and descriptions.

Returns an array of agents, each with:
- name: identifier for the agent
- description: summary of what the agent does

Use this to discover available agents before spawning them with spawn_agent."
    )]
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

    #[tool(description = "Spawn a sub-agent to perform a specific task.

Takes:
- agent_name: name of the agent from the sub-agents directory
- prompt: the task for the agent to perform
- model: optional model override (e.g., 'anthropic:claude-3.5-sonnet')

The agent executes the task and streams progress via MCP progress notifications.
These progress notifications show what the agent is doing in real-time but don't consume
the parent agent's context. When complete, returns the agent's final output as the tool result.")]
    pub async fn spawn_subagent(
        &self,
        request: Parameters<SpawnSubAgentInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<SpawnSubAgentOutput>, String> {
        let Parameters(args) = request;

        let agent_dir = self.agents_dir.join(&args.agent_name);
        if !agent_dir.exists() {
            return Err(format!("Agent '{}' not found", args.agent_name));
        }

        let agent_file_path = agent_dir.join("AGENTS.md");
        let agent_file = AgentFile::from_file(&agent_file_path)
            .map_err(|e| format!("Failed to load agent file: {e}"))?;

        let llm = self
            .create_llm(&args.agent_name, &agent_file, args.model)
            .await?;

        let (tools, mcp_tx, _mcp_handle) = self.spawn_mcps(&agent_dir).await?;
        let (user_tx, mut agent_rx, _agent_handle) = self
            .spawn_aether_agent(llm, &agent_file, mcp_tx, tools)
            .await?;

        user_tx
            .send(UserMessage::text(&args.prompt))
            .await
            .map_err(|e| format!("Failed to send message to agent: {}", e))?;

        let mut final_output = String::new();
        let mut message_counter = 0u64;
        let progress_token = context.meta.get_progress_token();

        while let Some(message) = agent_rx.recv().await {
            message_counter += 1;

            if let Some(ref token) = progress_token {
                let message_json = serde_json::to_string(&message)
                    .unwrap_or_else(|_| "Failed to serialize message".to_string());

                let _ = context
                    .peer
                    .notify_progress(ProgressNotificationParam {
                        progress_token: token.clone(),
                        progress: message_counter as f64,
                        total: None,
                        message: Some(message_json),
                    })
                    .await;
            }

            match &message {
                AgentMessage::Text {
                    chunk, is_complete, ..
                } => {
                    if *is_complete {
                        final_output = chunk.clone();
                    }
                }

                AgentMessage::Error { message } => {
                    final_output = format!("Error: {}", message);
                    break;
                }

                AgentMessage::Cancelled { message } => {
                    final_output = format!("Cancelled: {}", message);
                    break;
                }

                AgentMessage::Done => {
                    break;
                }

                _ => {
                    // All other message types (ToolCall, ToolProgress, ToolResult, ToolError)
                    // are already forwarded via progress notification above
                }
            }
        }

        Ok(Json(SpawnSubAgentOutput {
            output: final_output,
        }))
    }

    async fn create_llm(
        &self,
        agent_name: &str,
        agent_file: &AgentFile,
        model_override: Option<String>,
    ) -> Result<Box<dyn StreamingModelProvider>, String> {
        let model_spec = model_override
            .or_else(|| {
                agent_file
                    .frontmatter
                    .as_ref()
                    .map(|f| f.model.clone())
            })
            .ok_or_else(|| {
                format!(
                    "No model specified. Provide model parameter or set 'model' in {}/AGENTS.md frontmatter",
                    agent_name
                )
            })?;

        ModelProviderParser::default()
            .parse(&model_spec)
            .map_err(|e| format!("Failed to parse model spec '{}': {}", model_spec, e))
    }

    async fn spawn_mcps(
        &self,
        agent_dir: &Path,
    ) -> Result<
        (
            Vec<ToolDefinition>,
            mpsc::Sender<McpCommand>,
            JoinHandle<()>,
        ),
        String,
    > {
        let mcp_config_path = agent_dir.join("mcp.json");

        mcp()
            .register_in_memory_server("coding", Box::new(|_args| CodingMcp::new().into_dyn()))
            .from_json_file(mcp_config_path.to_str().unwrap_or(""))
            .map_err(|e| format!("Failed to load mcp.json: {}", e))?
            .spawn()
            .await
            .map_err(|e| format!("Failed to spawn MCP manager: {}", e))
    }

    async fn spawn_aether_agent(
        &self,
        llm: Box<dyn StreamingModelProvider>,
        agent_file: &AgentFile,
        mcp_tx: mpsc::Sender<McpCommand>,
        tools: Vec<ToolDefinition>,
    ) -> Result<
        (
            mpsc::Sender<UserMessage>,
            mpsc::Receiver<AgentMessage>,
            AgentHandle,
        ),
        String,
    > {
        let (user_tx, agent_rx, agent_handle) = agent(llm)
            .system(&agent_file.content)
            .tools(mcp_tx, tools)
            .spawn()
            .await
            .map_err(|e| format!("Failed to spawn agent: {}", e))?;

        Ok((user_tx, agent_rx, agent_handle))
    }
}
