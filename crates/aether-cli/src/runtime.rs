use crate::error::CliError;
use aether_core::agent_spec::{AgentSpec, McpConfigSource};
use aether_core::core::{AgentBuilder, AgentDeps, AgentHandle};
use aether_core::events::{AgentEvent, Command};
use aether_core::mcp::tool_bridge::tool_definitions;
use aether_core::mcp::{McpRuntime, mcp};
use llm::{ChatMessage, SessionUsageEvent, ToolDefinition};
use mcp_servers::McpBuilderExt;
use mcp_utils::client::Elicitation;
use mcp_utils::gateway::ServerSpec;
use std::path::PathBuf;
use tokio::sync::mpsc::{Receiver, Sender};
use tracing::debug;

pub struct RuntimeBuilder {
    cwd: PathBuf,
    spec: AgentSpec,
    mcp_config_sources: Vec<McpConfigSource>,
    extra_mcp_servers: Vec<ServerSpec>,
    elicitations: Option<Sender<Elicitation>>,
    wait_for_mcp: bool,
    agent_deps: AgentDeps,
    usage_seed: Option<SessionUsageEvent>,
}

pub struct Runtime {
    pub agent_tx: Sender<Command>,
    pub agent_rx: Receiver<AgentEvent>,
    pub agent_handle: AgentHandle,
    pub mcp_runtime: McpRuntime,
}

pub struct PromptInfo {
    pub spec: AgentSpec,
    pub tool_definitions: Vec<ToolDefinition>,
}

impl RuntimeBuilder {
    pub fn from_spec(cwd: PathBuf, spec: AgentSpec) -> Self {
        Self {
            cwd,
            spec,
            mcp_config_sources: Vec::new(),
            extra_mcp_servers: Vec::new(),
            elicitations: None,
            wait_for_mcp: false,
            agent_deps: AgentDeps::default(),
            usage_seed: None,
        }
    }

    pub fn agent_deps(mut self, deps: AgentDeps) -> Self {
        self.agent_deps = deps;
        self
    }

    /// Continue session usage totals from the last persisted usage event.
    pub fn resume_usage(mut self, last: SessionUsageEvent) -> Self {
        self.usage_seed = Some(last);
        self
    }

    /// Set MCP config source overrides. When non-empty, these completely
    /// replace any sources resolved from the agent's `AgentSpec`.
    pub fn mcp_sources(mut self, sources: Vec<McpConfigSource>) -> Self {
        self.mcp_config_sources = sources;
        self
    }

    pub fn extra_servers(mut self, servers: Vec<ServerSpec>) -> Self {
        self.extra_mcp_servers = servers;
        self
    }

    pub fn elicitations(mut self, sink: Sender<Elicitation>) -> Self {
        self.elicitations = Some(sink);
        self
    }

    pub fn wait_for_mcp(mut self) -> Self {
        self.wait_for_mcp = true;
        self
    }

    pub async fn build(self, messages: Vec<ChatMessage>) -> Result<Runtime, CliError> {
        let deps = self.agent_deps.clone();
        let usage_seed = self.usage_seed.clone();
        let wait_for_mcp = self.wait_for_mcp;
        let (spec, mcp_runtime) = self.spawn_mcp()?;
        if wait_for_mcp {
            mcp_runtime.gateway().ready().await;
        }

        let mut builder = AgentBuilder::from_spec(&spec, vec![], &deps)
            .await
            .map_err(|error| CliError::AgentError(error.to_string()))?
            .mcp(mcp_runtime.gateway().clone())
            .messages(messages);
        if let Some(last) = &usage_seed {
            builder = builder.resume_usage(last);
        }
        let (agent_tx, agent_rx, agent_handle) =
            builder.spawn().await.map_err(|error| CliError::AgentError(error.to_string()))?;

        Ok(Runtime { agent_tx, agent_rx, agent_handle, mcp_runtime })
    }

    pub async fn build_prompt_info(self) -> Result<PromptInfo, CliError> {
        let (spec, mcp_runtime) = self.spawn_mcp()?;
        let tool_definitions = tool_definitions(&*mcp_runtime.gateway().ready().await);
        Ok(PromptInfo { spec, tool_definitions })
    }

    fn spawn_mcp(self) -> Result<(AgentSpec, McpRuntime), CliError> {
        let mcp_config_sources: Vec<McpConfigSource> = if self.mcp_config_sources.is_empty() {
            self.spec.mcp_config_sources.clone()
        } else {
            self.mcp_config_sources
        };
        debug!("Loading MCP configs from: {:?}", mcp_config_sources);

        let mut builder = mcp(&self.cwd)
            .with_tool_filter(self.spec.tools.clone())
            .with_agent_deps(self.agent_deps)
            .with_builtin_servers()
            .with_servers(self.extra_mcp_servers)
            .from_mcp_config_sources(&mcp_config_sources)?;
        if let Some(sink) = self.elicitations {
            builder = builder.with_elicitations(sink);
        }
        Ok((self.spec, builder.spawn()?))
    }
}
