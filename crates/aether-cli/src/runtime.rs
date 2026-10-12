use aether_core::agent_spec::AgentSpec;
use aether_core::core::{AgentBuilder, AgentDeps, AgentError, AgentHandle, Prompt, PromptCache};
use aether_core::events::{AgentEvent, Command};
use aether_core::mcp::{McpCatalog, McpConfig, McpSpawnError, ParseError};
use llm::{ChatMessage, SessionUsageEvent, StreamingModelProvider, ToolDefinition};
use mcp_servers::agent_mcp;
use mcp_utils::client::ElicitationSink;
use mcp_utils::gateway::ServerSpec;
use std::path::PathBuf;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::mpsc::{Receiver, Sender};

pub struct RuntimeBuilder {
    cwd: PathBuf,
    spec: AgentSpec,
    provider: Option<Arc<dyn StreamingModelProvider>>,
    extra_mcp_servers: Vec<ServerSpec>,
    elicitation: Option<ElicitationSink>,
    wait_for_mcp: bool,
    agent_deps: AgentDeps,
    usage_seed: Option<SessionUsageEvent>,
}

pub struct Runtime {
    pub agent_tx: Sender<Command>,
    pub agent_rx: Receiver<AgentEvent>,
    pub agent_handle: AgentHandle,
    pub mcp: McpCatalog,
}

pub struct PromptPreview {
    pub system_prompt: String,
    pub tools: Vec<ToolDefinition>,
}

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("Invalid MCP config: {0}")]
    McpConfig(#[from] ParseError),
    #[error("MCP error: {0}")]
    McpSpawn(#[from] McpSpawnError),
    #[error("Agent error: {0}")]
    Agent(#[from] AgentError),
}

impl RuntimeBuilder {
    pub fn from_spec(cwd: PathBuf, spec: AgentSpec) -> Self {
        Self {
            cwd,
            spec,
            provider: None,
            extra_mcp_servers: Vec::new(),
            elicitation: None,
            wait_for_mcp: false,
            agent_deps: AgentDeps::default(),
            usage_seed: None,
        }
    }

    pub fn agent_deps(mut self, deps: AgentDeps) -> Self {
        self.agent_deps = deps;
        self
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn provider(mut self, provider: Arc<dyn StreamingModelProvider>) -> Self {
        self.provider = Some(provider);
        self
    }

    pub fn resume_usage(mut self, last: Option<SessionUsageEvent>) -> Self {
        self.usage_seed = last;
        self
    }

    /// Replaces the agent's own MCP servers when `config` names any.
    pub fn mcp_config(mut self, config: McpConfig) -> Self {
        if !config.is_empty() {
            self.spec.mcp = config;
        }
        self
    }

    pub fn extra_servers(mut self, servers: Vec<ServerSpec>) -> Self {
        self.extra_mcp_servers = servers;
        self
    }

    pub fn elicitation(mut self, sink: ElicitationSink) -> Self {
        self.elicitation = Some(sink);
        self
    }

    pub fn wait_for_mcp(mut self) -> Self {
        self.wait_for_mcp = true;
        self
    }

    pub async fn build(self, messages: Vec<ChatMessage>) -> Result<Runtime, RuntimeError> {
        let mcp = self.spawn_mcp()?;
        if self.wait_for_mcp {
            mcp.ready().await;
        }

        let builder = match self.provider.clone() {
            Some(provider) => AgentBuilder::from_spec_with_llm(&self.spec, &self.agent_deps, provider),
            None => AgentBuilder::from_spec(&self.spec, &self.agent_deps).await?,
        };
        let mut builder = builder.mcp(mcp.clone()).messages(messages);
        if let Some(last) = &self.usage_seed {
            builder = builder.resume_usage(last);
        }
        let (agent_tx, agent_rx, agent_handle) = builder.spawn().await?;

        Ok(Runtime { agent_tx, agent_rx, agent_handle, mcp })
    }

    pub async fn preview(self) -> Result<PromptPreview, RuntimeError> {
        let mcp = self.spawn_mcp()?;
        mcp.ready().await;
        let mut prompts = self.spec.prompts.clone();
        prompts.push(Prompt::McpInstructions(mcp.instructions()));
        let system_prompt = PromptCache::new(prompts).render().await;
        let tools = mcp.tool_definitions();
        mcp.shutdown().await;
        Ok(PromptPreview { system_prompt: system_prompt?, tools })
    }

    fn spawn_mcp(&self) -> Result<McpCatalog, RuntimeError> {
        let mut builder = agent_mcp(&self.cwd, self.agent_deps.clone(), self.spec.tools.clone())
            .with_servers(self.extra_mcp_servers.clone())
            .with_config(self.spec.mcp.clone())?;
        if let Some(sink) = self.elicitation.clone() {
            builder = builder.with_elicitation(sink);
        }
        Ok(builder.spawn()?)
    }
}

impl Runtime {
    pub async fn shutdown(self) {
        let Self { agent_tx, agent_rx, agent_handle, mcp } = self;
        drop((agent_tx, agent_rx));
        agent_handle.abort();
        agent_handle.await_completion().await;
        mcp.shutdown().await;
    }
}
