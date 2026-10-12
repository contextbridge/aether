use super::error::SessionError;
use crate::runtime::{Runtime, RuntimeBuilder};
use aether_core::agent_spec::AgentSpec;
use aether_core::core::AgentDeps;
use aether_core::events::{AgentCommand, AgentEvent, Command};
use aether_core::mcp::McpCatalog;
use futures::Stream;
use llm::{ChatMessage, SessionUsageEvent};
use mcp_utils::client::{Elicitation, ElicitationSink};
use mcp_utils::gateway::ServerSpec;
use mcp_utils::model::ElicitationCapability;
use std::path::PathBuf;
use std::pin::Pin;
use tokio::sync::mpsc;

pub(crate) struct AgentRuntime {
    runtime: Runtime,
    mcp_changes: McpChanges,
}

pub(crate) type McpChanges = Pin<Box<dyn Stream<Item = ()> + Send + Sync>>;

impl AgentRuntime {
    pub(crate) fn new(runtime: Runtime) -> Self {
        let mcp_changes = Box::pin(runtime.mcp.changes());
        Self { runtime, mcp_changes }
    }

    pub(crate) fn events(&mut self) -> (&mut mpsc::Receiver<AgentEvent>, &mut McpChanges) {
        (&mut self.runtime.agent_rx, &mut self.mcp_changes)
    }

    pub(crate) async fn shutdown(self) {
        self.runtime.shutdown().await;
    }

    pub(crate) async fn send_agent_command(&self, command: Command) -> Result<(), SessionError> {
        self.runtime.agent_tx.send(command).await.map_err(|_| SessionError::CommandChannelClosed)
    }

    pub(crate) async fn replace_conversation(&self, messages: Vec<ChatMessage>) -> Result<(), SessionError> {
        self.send_agent_command(Command::agent(AgentCommand::ReplaceConversation(messages))).await
    }

    pub(crate) fn mcp(&self) -> &McpCatalog {
        &self.runtime.mcp
    }
}

/// Spawns the [`AgentRuntime`] backing a session's agent. Production uses
/// [`ProductionRuntimeFactory`]; tests substitute their own implementation so a
/// session can run end-to-end against fake LLMs and in-memory MCP servers.
#[async_trait::async_trait]
pub(crate) trait RuntimeFactory: Send + Sync {
    async fn spawn(
        &self,
        spec: &AgentSpec,
        initial_messages: Vec<ChatMessage>,
        usage_seed: Option<SessionUsageEvent>,
        elicitations: mpsc::Sender<Elicitation>,
    ) -> Result<AgentRuntime, SessionError>;
}

pub(crate) struct ProductionRuntimeFactory {
    cwd: PathBuf,
    mcp_servers: Vec<ServerSpec>,
    agent_deps: AgentDeps,
    elicitation: ElicitationCapability,
}

impl ProductionRuntimeFactory {
    pub fn new(
        cwd: PathBuf,
        mcp_servers: Vec<ServerSpec>,
        agent_deps: AgentDeps,
        elicitation: ElicitationCapability,
    ) -> Self {
        Self { cwd, mcp_servers, agent_deps, elicitation }
    }
}

#[async_trait::async_trait]
impl RuntimeFactory for ProductionRuntimeFactory {
    async fn spawn(
        &self,
        spec: &AgentSpec,
        initial_messages: Vec<ChatMessage>,
        usage_seed: Option<SessionUsageEvent>,
        elicitations: mpsc::Sender<Elicitation>,
    ) -> Result<AgentRuntime, SessionError> {
        let sink = ElicitationSink::new(elicitations, self.elicitation.clone());
        let runtime = RuntimeBuilder::from_spec(self.cwd.clone(), spec.clone())
            .extra_servers(self.mcp_servers.clone())
            .elicitation(sink)
            .agent_deps(self.agent_deps.clone())
            .resume_usage(usage_seed)
            .build(initial_messages)
            .await?;
        Ok(AgentRuntime::new(runtime))
    }
}
