use super::agent_key::AgentKey;
use super::error::SessionError;
use crate::runtime::{Runtime, RuntimeBuilder};
use aether_auth::OAuthHandler;
use aether_core::agent_spec::AgentSpec;
use aether_core::core::{AgentDeps, AgentHandle};
use aether_core::events::{AgentCommand, AgentEvent, Command};
use aether_core::mcp::{McpHandle, McpRuntime};
use llm::{ChatMessage, SessionUsageEvent};
use mcp_utils::client::{
    ElicitingOAuthHandler, McpClientEvent, McpConnectionDetails, McpError, McpServer, McpServerStatusEntry,
    OAuthHandlerFactory,
};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{mpsc, watch};

pub(crate) struct AgentRuntime {
    pub(crate) agent_rx: mpsc::Receiver<AgentEvent>,
    pub(crate) event_rx: mpsc::Receiver<McpClientEvent>,
    agent_tx: mpsc::Sender<Command>,
    latest_mcp_snapshot: watch::Receiver<McpConnectionDetails>,
    agent_handle: Option<AgentHandle>,
    mcp_runtime: McpRuntime,
}

impl AgentRuntime {
    pub(crate) fn new(
        agent_tx: mpsc::Sender<Command>,
        agent_rx: mpsc::Receiver<AgentEvent>,
        agent_handle: Option<AgentHandle>,
        event_rx: mpsc::Receiver<McpClientEvent>,
        mcp_runtime: McpRuntime,
    ) -> Self {
        let latest_mcp_snapshot = mcp_runtime.handle().subscribe();
        Self { agent_rx, event_rx, agent_tx, latest_mcp_snapshot, agent_handle, mcp_runtime }
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

    pub(crate) fn mcp(&self) -> &McpHandle {
        self.mcp_runtime.handle()
    }

    pub(crate) fn mcp_server_statuses(&self) -> Vec<McpServerStatusEntry> {
        self.latest_mcp_snapshot.borrow().server_statuses()
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
    mcp_servers: Vec<McpServer>,
    agent_deps: AgentDeps,
}

impl ProductionRuntimeFactory {
    pub fn new(cwd: PathBuf, client_servers: Vec<McpServer>, agent_deps: AgentDeps) -> Self {
        Self { cwd, mcp_servers: client_servers, agent_deps }
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

        let mut builder = RuntimeBuilder::from_spec(self.cwd.clone(), spec.clone())
            .extra_servers(extra_servers)
            .agent_deps(self.agent_deps.clone());
        if let Some(last) = usage_seed {
            builder = builder.resume_usage(last);
        }
        if self.agent_deps.supports_mcp_url_elicitation() {
            builder = builder.oauth_handler_factory(mcp_oauth_handler_factory());
        }

        let runtime = builder.build(None, Some(initial_messages)).await?;

        let Runtime { agent_tx, agent_rx, agent_handle, event_rx, mcp_runtime } = runtime;
        Ok(AgentRuntime::new(agent_tx, agent_rx, Some(agent_handle), event_rx, mcp_runtime))
    }
}

fn mcp_oauth_handler_factory() -> OAuthHandlerFactory {
    Arc::new(|ctx| {
        ElicitingOAuthHandler::new(ctx)
            .map(|handler| Arc::new(handler) as Arc<dyn OAuthHandler>)
            .map_err(|error| McpError::ConnectionFailed(format!("failed to initialize OAuth handler: {error}")))
    })
}
