use crate::error::CliError;
use aether_core::agent_spec::{AgentSpec, McpConfigSource};
use aether_core::core::{AgentBuilder, AgentHandle, Prompt};
use aether_core::events::{AgentMessage, UserMessage};
use aether_core::mcp::McpBuilder;
use aether_core::mcp::McpSpawnResult;
use aether_core::mcp::mcp;
use aether_core::mcp::run_mcp_task::McpCommand;
use llm::{ChatMessage, LlmModel, ToolDefinition};
use mcp_servers::McpBuilderExt;
use mcp_utils::client::oauth::OAuthHandler;
use mcp_utils::client::{McpClientEvent, McpServerConfig};
use mcp_utils::status::McpServerStatusEntry;
use std::path::{Path, PathBuf};
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::task::JoinHandle;
use tracing::debug;

pub struct RuntimeBuilder {
    cwd: PathBuf,
    spec: AgentSpec,
    mcp_config_sources: Vec<McpConfigSource>,
    extra_mcp_servers: Vec<McpServerConfig>,
    oauth_applicator: Option<Box<dyn FnOnce(McpBuilder) -> McpBuilder + Send>>,
    prompt_cache_key: Option<String>,
}

pub struct Runtime {
    pub agent_tx: Sender<UserMessage>,
    pub agent_rx: Receiver<AgentMessage>,
    pub agent_handle: AgentHandle,
    pub mcp_tx: Sender<McpCommand>,
    pub event_rx: Receiver<McpClientEvent>,
    pub server_statuses: Vec<McpServerStatusEntry>,
    pub mcp_handle: JoinHandle<()>,
}

pub struct PromptInfo {
    pub spec: AgentSpec,
    pub tool_definitions: Vec<ToolDefinition>,
}

impl RuntimeBuilder {
    pub fn new(cwd: &Path, model: &str) -> Result<Self, CliError> {
        let cwd = cwd.canonicalize().map_err(CliError::IoError)?;
        let parsed_model: LlmModel = model.parse().map_err(CliError::ModelError)?;
        let spec = AgentSpec::default_spec(&parsed_model, None, Vec::new());

        Ok(Self {
            cwd,
            spec,
            mcp_config_sources: Vec::new(),
            extra_mcp_servers: Vec::new(),
            oauth_applicator: None,
            prompt_cache_key: None,
        })
    }

    pub fn from_spec(cwd: PathBuf, spec: AgentSpec) -> Self {
        Self {
            cwd,
            spec,
            mcp_config_sources: Vec::new(),
            extra_mcp_servers: Vec::new(),
            oauth_applicator: None,
            prompt_cache_key: None,
        }
    }

    pub fn prompt_cache_key(mut self, key: String) -> Self {
        self.prompt_cache_key = Some(key);
        self
    }

    /// Set MCP config source overrides. When non-empty, these completely
    /// replace any sources resolved from the agent's `AgentSpec`.
    pub fn mcp_sources(mut self, sources: Vec<McpConfigSource>) -> Self {
        self.mcp_config_sources = sources;
        self
    }

    pub fn extra_servers(mut self, servers: Vec<McpServerConfig>) -> Self {
        self.extra_mcp_servers = servers;
        self
    }

    pub fn oauth_handler<H: OAuthHandler + 'static>(mut self, handler: H) -> Self {
        self.oauth_applicator = Some(Box::new(|builder| builder.with_oauth_handler(handler)));
        self
    }

    pub async fn build(
        self,
        custom_prompt: Option<Prompt>,
        messages: Option<Vec<ChatMessage>>,
    ) -> Result<Runtime, CliError> {
        let prompt_cache_key = self.prompt_cache_key.clone();
        let mcp = self.spawn_mcp().await?;

        let filtered_tools = mcp.spec.tools.apply(mcp.tool_definitions);
        let mut agent_builder = AgentBuilder::from_spec(&mcp.spec, vec![])
            .await
            .map_err(|e| CliError::AgentError(e.to_string()))?
            .tools(mcp.mcp_tx.clone(), filtered_tools);

        if let Some(key) = prompt_cache_key {
            agent_builder = agent_builder.prompt_cache_key(key);
        }

        if let Some(prompt) = custom_prompt {
            agent_builder = agent_builder.system_prompt(prompt);
        }

        if let Some(msgs) = messages {
            agent_builder = agent_builder.messages(msgs);
        }

        let (agent_tx, agent_rx, agent_handle) =
            agent_builder.spawn().await.map_err(|e| CliError::AgentError(e.to_string()))?;

        Ok(Runtime {
            agent_tx,
            agent_rx,
            agent_handle,
            mcp_tx: mcp.mcp_tx,
            event_rx: mcp.event_rx,
            server_statuses: mcp.server_statuses,
            mcp_handle: mcp.mcp_handle,
        })
    }

    pub async fn build_prompt_info(self) -> Result<PromptInfo, CliError> {
        let mcp = self.spawn_mcp().await?;
        let filtered_tools = mcp.spec.tools.apply(mcp.tool_definitions);
        Ok(PromptInfo { spec: mcp.spec, tool_definitions: filtered_tools })
    }

    async fn spawn_mcp(self) -> Result<McpParts, CliError> {
        let mut builder = mcp().with_builtin_servers(self.cwd.clone(), &self.cwd);

        if !self.extra_mcp_servers.is_empty() {
            builder = builder.with_servers(self.extra_mcp_servers);
        }

        if let Some(apply_oauth) = self.oauth_applicator {
            builder = apply_oauth(builder);
        }

        let mcp_config_sources: Vec<McpConfigSource> = if self.mcp_config_sources.is_empty() {
            self.spec.mcp_config_sources.clone()
        } else {
            self.mcp_config_sources
        };

        if !mcp_config_sources.is_empty() {
            debug!("Loading MCP configs from: {:?}", mcp_config_sources);
            builder = builder
                .from_mcp_config_sources(&mcp_config_sources)
                .await
                .map_err(|e| CliError::McpError(e.to_string()))?;
        }

        let McpSpawnResult {
            tool_definitions,
            instructions,
            server_statuses,
            command_tx: mcp_tx,
            event_rx,
            handle: mcp_handle,
        } = builder.spawn().await.map_err(|e| CliError::McpError(e.to_string()))?;

        let mut spec = self.spec;
        spec.prompts.push(Prompt::mcp_instructions(instructions));

        Ok(McpParts { spec, tool_definitions, mcp_tx, event_rx, server_statuses, mcp_handle })
    }
}

struct McpParts {
    spec: AgentSpec,
    tool_definitions: Vec<ToolDefinition>,
    mcp_tx: Sender<McpCommand>,
    event_rx: Receiver<McpClientEvent>,
    server_statuses: Vec<McpServerStatusEntry>,
    mcp_handle: JoinHandle<()>,
}
