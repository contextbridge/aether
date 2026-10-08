use super::agent_key::AgentKey;
use super::error::SessionError;
use crate::runtime::{Runtime, RuntimeBuilder};
use aether_core::agent_spec::AgentSpec;
use aether_core::core::{AgentDeps, AgentHandle};
use aether_core::events::{AgentCommand, AgentEvent, Command};
use aether_core::mcp::McpRuntime;
use llm::{ChatMessage, SessionUsageEvent};
use mcp_utils::client::Elicitation;
use mcp_utils::gateway::{McpCatalog, McpGateway, ServerSpec};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{mpsc, watch};
use utils::mcp_status::McpServerStatusEntry;

pub(crate) struct AgentRuntime {
    pub(crate) agent_rx: mpsc::Receiver<AgentEvent>,
    pub(crate) event_rx: mpsc::Receiver<Elicitation>,
    pub(crate) mcp_catalog: watch::Receiver<Arc<McpCatalog>>,
    agent_tx: mpsc::Sender<Command>,
    agent_handle: Option<AgentHandle>,
    mcp_runtime: McpRuntime,
    reported_statuses: Vec<McpServerStatusEntry>,
}

impl AgentRuntime {
    pub(crate) fn new(
        agent_tx: mpsc::Sender<Command>,
        agent_rx: mpsc::Receiver<AgentEvent>,
        agent_handle: Option<AgentHandle>,
        event_rx: mpsc::Receiver<Elicitation>,
        mcp_runtime: McpRuntime,
    ) -> Self {
        let mcp_catalog = mcp_runtime.gateway().subscribe();
        Self { agent_rx, event_rx, mcp_catalog, agent_tx, agent_handle, mcp_runtime, reported_statuses: Vec::new() }
    }

    pub(crate) async fn shutdown(mut self) {
        if let Some(handle) = self.agent_handle.take() {
            handle.abort();
            handle.await_completion().await;
        }
        self.mcp_runtime.shutdown().await;
    }

    pub(crate) async fn send_agent_command(&self, command: Command) -> Result<(), SessionError> {
        self.agent_tx.send(command).await.map_err(|_| SessionError::CommandChannelClosed)
    }

    pub(crate) async fn replace_conversation(&self, messages: Vec<ChatMessage>) -> Result<(), SessionError> {
        self.agent_tx
            .send(Command::agent(AgentCommand::ReplaceConversation(messages)))
            .await
            .map_err(|_| SessionError::CommandChannelClosed)
    }

    pub(crate) fn mcp(&self) -> &McpGateway {
        self.mcp_runtime.gateway()
    }

    pub(crate) fn report_statuses(&mut self) -> Vec<McpServerStatusEntry> {
        self.take_status_change();
        self.reported_statuses.clone()
    }

    pub(crate) fn take_status_change(&mut self) -> Option<Vec<McpServerStatusEntry>> {
        let statuses = self.mcp_catalog.borrow_and_update().statuses();
        if statuses == self.reported_statuses {
            return None;
        }
        self.reported_statuses.clone_from(&statuses);
        Some(statuses)
    }
}

impl Drop for AgentRuntime {
    fn drop(&mut self) {
        if let Some(handle) = &self.agent_handle {
            handle.abort();
        }
    }
}

/// Spawns the [`AgentRuntime`] backing a session's agent. Production uses
/// [`ProductionRuntimeFactory`]; tests substitute their own implementation so a
/// session can run end-to-end against fake LLMs and in-memory MCP servers.
#[async_trait::async_trait]
pub(crate) trait RuntimeFactory: Send + Sync {
    async fn spawn(
        &self,
        agent: AgentKey,
        spec: &AgentSpec,
        initial_messages: Vec<ChatMessage>,
        usage_seed: Option<SessionUsageEvent>,
    ) -> Result<AgentRuntime, SessionError>;
}

pub(crate) struct ProductionRuntimeFactory {
    cwd: PathBuf,
    mcp_servers: Vec<ServerSpec>,
    agent_deps: AgentDeps,
}

impl ProductionRuntimeFactory {
    pub fn new(cwd: PathBuf, mcp_servers: Vec<ServerSpec>, agent_deps: AgentDeps) -> Self {
        Self { cwd, mcp_servers, agent_deps }
    }
}

#[async_trait::async_trait]
impl RuntimeFactory for ProductionRuntimeFactory {
    async fn spawn(
        &self,
        _agent: AgentKey,
        spec: &AgentSpec,
        initial_messages: Vec<ChatMessage>,
        usage_seed: Option<SessionUsageEvent>,
    ) -> Result<AgentRuntime, SessionError> {
        let extra_servers = self.mcp_servers.clone();

        let (elicitations, event_rx) = mpsc::channel(ELICITATION_CAPACITY);
        let mut builder = RuntimeBuilder::from_spec(self.cwd.clone(), spec.clone())
            .extra_servers(extra_servers)
            .elicitations(elicitations)
            .agent_deps(self.agent_deps.clone());
        if let Some(last) = usage_seed {
            builder = builder.resume_usage(last);
        }

        let runtime = builder.build(initial_messages).await?;

        let Runtime { agent_tx, agent_rx, agent_handle, mcp_runtime } = runtime;
        Ok(AgentRuntime::new(agent_tx, agent_rx, Some(agent_handle), event_rx, mcp_runtime))
    }
}

const ELICITATION_CAPACITY: usize = 32;
