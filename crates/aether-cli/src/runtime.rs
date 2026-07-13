use crate::error::CliError;
use aether_core::agent_spec::{AgentSpec, McpConfigSource};
use aether_core::core::{AgentBuilder, AgentDeps, AgentHandle, Prompt};
use aether_core::events::{AgentEvent, Command};
use aether_core::mcp::McpBuilder;
use aether_core::mcp::McpSpawnResult;
use aether_core::mcp::mcp;
use aether_core::mcp::run_mcp_task::McpCommand;
use llm::{ChatMessage, LlmModel, ToolDefinition};
use mcp_servers::McpBuilderExt;
use mcp_utils::client::{McpClientEvent, McpConnectionDetails, McpServer, OAuthHandlerFactory};
use std::path::{Path, PathBuf};
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::task::JoinHandle;
use tracing::debug;

pub struct RuntimeBuilder {
    cwd: PathBuf,
    spec: AgentSpec,
    mcp_config_sources: Vec<McpConfigSource>,
    extra_mcp_servers: Vec<McpServer>,
    oauth_applicator: Option<Box<dyn FnOnce(McpBuilder) -> McpBuilder + Send>>,
    agent_deps: AgentDeps,
    prompt_cache_key: Option<String>,
}

pub struct Runtime {
    pub agent_tx: Sender<Command>,
    pub agent_rx: Receiver<AgentEvent>,
    pub agent_handle: AgentHandle,
    pub mcp_tx: Sender<McpCommand>,
    pub event_rx: Receiver<McpClientEvent>,
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
            agent_deps: AgentDeps::default(),
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
            agent_deps: AgentDeps::default(),
            prompt_cache_key: None,
        }
    }

    pub fn prompt_cache_key(mut self, key: String) -> Self {
        self.prompt_cache_key = Some(key);
        self
    }

    pub fn agent_deps(mut self, deps: AgentDeps) -> Self {
        self.agent_deps = deps;
        self
    }

    /// Set MCP config source overrides. When non-empty, these completely
    /// replace any sources resolved from the agent's `AgentSpec`.
    pub fn mcp_sources(mut self, sources: Vec<McpConfigSource>) -> Self {
        self.mcp_config_sources = sources;
        self
    }

    pub fn extra_servers(mut self, servers: Vec<McpServer>) -> Self {
        self.extra_mcp_servers = servers;
        self
    }

    pub fn oauth_handler_factory(mut self, factory: OAuthHandlerFactory) -> Self {
        self.oauth_applicator = Some(Box::new(|builder| builder.with_oauth_handler_factory(factory)));
        self
    }

    pub async fn build(
        self,
        custom_prompt: Option<Prompt>,
        messages: Option<Vec<ChatMessage>>,
    ) -> Result<Runtime, CliError> {
        let deps = self.agent_deps.clone();
        let prompt_cache_key = self.prompt_cache_key.clone();
        let (spec, spawn) = self.spawn_mcp().await?;
        let McpSpawnResult { command_tx: mcp_tx, event_rx, handle: mcp_handle } = spawn;

        let (agent_tx, agent_rx, agent_handle) =
            spawn_agent(&spec, &deps, mcp_tx.clone(), Vec::new(), prompt_cache_key, |mut agent_builder| {
                if let Some(prompt) = custom_prompt {
                    agent_builder = agent_builder.system_prompt(prompt);
                }
                if let Some(msgs) = messages {
                    agent_builder = agent_builder.messages(msgs);
                }
                agent_builder
            })
            .await?;

        Ok(Runtime { agent_tx, agent_rx, agent_handle, mcp_tx, event_rx, mcp_handle })
    }

    /// Spawn MCP, block until every server finishes its initial connection,
    /// then build an agent pre-seeded with the connection's filtered tools and
    /// MCP instructions. Returns the live [`Runtime`] plus the bootstrap
    /// snapshot. Unlike [`build`](Self::build) (which starts with no tools and
    /// relies on the MCP event stream to populate them later), this is for
    /// callers that need the agent ready to use tools on its first turn.
    pub async fn build_ready(self, messages: Vec<ChatMessage>) -> Result<(Runtime, McpConnectionDetails), CliError> {
        let deps = self.agent_deps.clone();
        let prompt_cache_key = self.prompt_cache_key.clone();
        let (spec, mut spawn) = self.spawn_mcp().await?;
        let snapshot = spawn
            .block_until_ready()
            .await
            .ok_or_else(|| CliError::McpError("MCP bootstrap aborted before completion".to_string()))?;
        let McpSpawnResult { command_tx: mcp_tx, event_rx, handle: mcp_handle } = spawn;

        let filtered_tools = spec.tools.apply(snapshot.tool_definitions.clone());
        let mut runtime_spec = spec;
        runtime_spec.prompts.push(Prompt::McpInstructions(snapshot.instructions.clone()));

        let (agent_tx, agent_rx, agent_handle) =
            spawn_agent(&runtime_spec, &deps, mcp_tx.clone(), filtered_tools, prompt_cache_key, |agent_builder| {
                agent_builder.messages(messages)
            })
            .await?;

        Ok((Runtime { agent_tx, agent_rx, agent_handle, mcp_tx, event_rx, mcp_handle }, snapshot))
    }

    pub async fn build_prompt_info(self) -> Result<PromptInfo, CliError> {
        let (spec, mut spawn) = self.spawn_mcp().await?;
        let details = spawn
            .block_until_ready()
            .await
            .ok_or_else(|| CliError::McpError("MCP bootstrap aborted before completion".to_string()))?;
        let filtered_tools = spec.tools.apply(details.tool_definitions);
        Ok(PromptInfo { spec, tool_definitions: filtered_tools })
    }

    async fn spawn_mcp(self) -> Result<(AgentSpec, McpSpawnResult), CliError> {
        let mut builder = mcp(&self.cwd).with_agent_deps(self.agent_deps.clone());

        if let Some(apply_oauth) = self.oauth_applicator {
            builder = apply_oauth(builder);
        }

        builder = builder.with_builtin_servers();

        if !self.extra_mcp_servers.is_empty() {
            builder = builder.with_servers(self.extra_mcp_servers);
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

        let spawn = builder.spawn().await.map_err(|e| CliError::McpError(e.to_string()))?;
        Ok((self.spec, spawn))
    }
}

async fn spawn_agent(
    spec: &AgentSpec,
    deps: &AgentDeps,
    mcp_tx: Sender<McpCommand>,
    tool_definitions: Vec<ToolDefinition>,
    prompt_cache_key: Option<String>,
    configure: impl FnOnce(AgentBuilder) -> AgentBuilder,
) -> Result<(Sender<Command>, Receiver<AgentEvent>, AgentHandle), CliError> {
    let mut builder = AgentBuilder::from_spec(spec, vec![], deps)
        .await
        .map_err(|error| CliError::AgentError(error.to_string()))?
        .tools(mcp_tx, tool_definitions);

    if let Some(key) = prompt_cache_key {
        builder = builder.prompt_cache_key(key);
    }

    configure(builder).spawn().await.map_err(|error| CliError::AgentError(error.to_string()))
}
