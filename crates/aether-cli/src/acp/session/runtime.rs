use super::agent_key::AgentKey;
use super::error::SessionError;
use crate::runtime::{Runtime, RuntimeBuilder};
use crate::slash_commands::list_prompts;
use aether_auth::OAuthHandler;
use aether_core::agent_spec::AgentSpec;
use aether_core::core::{AgentDeps, AgentHandle};
use aether_core::events::{AgentCommand, AgentEvent, Command};
use aether_core::mcp::{McpHandle, McpRuntime};
use llm::ChatMessage;
use mcp_utils::client::{
    ElicitingOAuthHandler, McpClientEvent, McpConnectionDetails, McpError, McpServer, McpServerStatusEntry,
    OAuthHandlerFactory,
};
use rmcp::model::Prompt as McpPrompt;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

/// Capacity of the channel that fans runtime events from every spawned agent
/// into the session actor relay loop.
pub(crate) const RUNTIME_EVENT_CHANNEL_CAPACITY: usize = 50;

pub(crate) struct AgentRuntime {
    agent_tx: mpsc::Sender<Command>,
    latest_mcp_snapshot: watch::Receiver<McpConnectionDetails>,
    agent_handle: Option<AgentHandle>,
    mcp_runtime: McpRuntime,
    agent_pump_handle: JoinHandle<()>,
    mcp_pump_handle: JoinHandle<()>,
}

impl AgentRuntime {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        agent: AgentKey,
        agent_tx: mpsc::Sender<Command>,
        mut agent_rx: mpsc::Receiver<AgentEvent>,
        agent_handle: Option<AgentHandle>,
        mut event_rx: mpsc::Receiver<McpClientEvent>,
        mcp_runtime: McpRuntime,
        runtime_event_tx: mpsc::Sender<RuntimeEvent>,
    ) -> Self {
        let latest_mcp_snapshot = mcp_runtime.handle().subscribe();
        let agent_event_tx = runtime_event_tx.clone();
        let agent_event_key = agent.clone();
        let agent_pump_handle = tokio::spawn(async move {
            while let Some(message) = agent_rx.recv().await {
                if agent_event_tx.send(RuntimeEvent::Agent { agent: agent_event_key.clone(), message }).await.is_err() {
                    break;
                }
            }
        });

        let mcp_pump_handle = tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                if runtime_event_tx.send(RuntimeEvent::Mcp { agent: agent.clone(), event }).await.is_err() {
                    break;
                }
            }
        });

        Self { agent_tx, latest_mcp_snapshot, agent_handle, mcp_runtime, agent_pump_handle, mcp_pump_handle }
    }

    pub(crate) async fn send_agent_command(&self, command: Command) -> Result<(), SessionError> {
        self.agent_tx
            .send(command)
            .await
            .map_err(|e| SessionError::CommandChannel(format!("failed to send agent command: {e}")))
    }

    pub(crate) async fn replace_conversation(&self, messages: Vec<ChatMessage>) -> Result<(), SessionError> {
        self.agent_tx
            .send(Command::agent(AgentCommand::ReplaceConversation(messages)))
            .await
            .map_err(|e| SessionError::CommandChannel(format!("failed to sync active conversation: {e}")))
    }

    pub(crate) fn mcp(&self) -> &McpHandle {
        self.mcp_runtime.handle()
    }

    pub(crate) async fn list_prompts(&self) -> Result<Vec<McpPrompt>, SessionError> {
        list_prompts(self.mcp()).await.map_err(|error| SessionError::McpOperation(error.to_string()))
    }

    pub(crate) async fn authenticate_mcp_server(&self, name: &str) -> Result<(), SessionError> {
        self.mcp().authenticate_server(name).await.map_err(|error| SessionError::McpOperation(error.to_string()))
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
        self.agent_pump_handle.abort();
        self.mcp_pump_handle.abort();
    }
}

#[allow(clippy::large_enum_variant)]
pub(crate) enum RuntimeEvent {
    Agent { agent: AgentKey, message: AgentEvent },
    Mcp { agent: AgentKey, event: McpClientEvent },
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
        runtime_event_tx: mpsc::Sender<RuntimeEvent>,
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
        agent: AgentKey,
        spec: &AgentSpec,
        initial_messages: Vec<ChatMessage>,
        runtime_event_tx: mpsc::Sender<RuntimeEvent>,
    ) -> Result<AgentRuntime, SessionError> {
        let extra_servers = self.mcp_servers.clone();

        let mut builder = RuntimeBuilder::from_spec(self.cwd.clone(), spec.clone())
            .extra_servers(extra_servers)
            .agent_deps(self.agent_deps.clone());
        if self.agent_deps.supports_mcp_url_elicitation() {
            builder = builder.oauth_handler_factory(mcp_oauth_handler_factory());
        }

        let runtime = builder.build(None, Some(initial_messages)).await?;

        let Runtime { agent_tx, agent_rx, agent_handle, event_rx, mcp_runtime } = runtime;
        Ok(AgentRuntime::new(agent, agent_tx, agent_rx, Some(agent_handle), event_rx, mcp_runtime, runtime_event_tx))
    }
}

fn mcp_oauth_handler_factory() -> OAuthHandlerFactory {
    Arc::new(|ctx| {
        ElicitingOAuthHandler::new(ctx)
            .map(|handler| Arc::new(handler) as Arc<dyn OAuthHandler>)
            .map_err(|error| McpError::ConnectionFailed(format!("failed to initialize OAuth handler: {error}")))
    })
}
