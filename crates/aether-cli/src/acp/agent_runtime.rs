use super::agent_key::AgentKey;
use super::error::SessionError;
use crate::runtime::{Runtime, RuntimeBuilder};
use crate::slash_commands::{SlashCommandError, list_prompts};
use aether_auth::OAuthHandler;
use aether_core::agent_spec::AgentSpec;
use aether_core::core::{AgentDeps, AgentHandle};
use aether_core::events::{AgentCommand, AgentEvent, Command};
use aether_core::mcp::{McpRuntime, run_mcp_task::McpCommand};
use llm::ChatMessage;
use mcp_utils::client::{
    ElicitingOAuthHandler, McpClientEvent, McpConnectionDetails, McpError, McpServer, McpServerStatusEntry,
    OAuthHandlerFactory, ToolCatalog,
};
use rmcp::model::Prompt as McpPrompt;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

/// Capacity of the channel that fans runtime events from every spawned agent
/// into the single relay loop. Both ends (`session_manager` and the test
/// harness) must agree, so the value lives here.
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
        snapshot: McpConnectionDetails,
        runtime_event_tx: mpsc::Sender<RuntimeEvent>,
    ) -> Self {
        let (latest_mcp_snapshot_tx, latest_mcp_snapshot) = watch::channel(snapshot);
        let agent_event_tx = runtime_event_tx.clone();
        let agent_event_key = agent.clone();
        let agent_pump_handle = tokio::spawn(async move {
            while let Some(message) = agent_rx.recv().await {
                if agent_event_tx.send(RuntimeEvent::Agent { agent: agent_event_key.clone(), message }).await.is_err() {
                    break;
                }
            }
        });

        let mcp_agent_tx = agent_tx.clone();
        let mcp_pump_handle = tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                let Some(relay_event) = on_mcp_event(event, &latest_mcp_snapshot_tx, &mcp_agent_tx).await else {
                    continue;
                };
                if runtime_event_tx.send(RuntimeEvent::Mcp { agent: agent.clone(), event: relay_event }).await.is_err()
                {
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

    pub(crate) fn mcp_tx(&self) -> &mpsc::Sender<McpCommand> {
        self.mcp_runtime.command_tx()
    }

    pub(crate) async fn list_prompts(&self) -> Result<Vec<McpPrompt>, SessionError> {
        list_prompts(self.mcp_tx()).await.map_err(|error| match error {
            SlashCommandError::CommandChannel(message) => SessionError::CommandChannel(message),
            other => SessionError::McpOperation(other.to_string()),
        })
    }

    pub(crate) async fn authenticate_mcp_server(&self, name: &str) -> Result<(), SessionError> {
        self.mcp_tx()
            .send(McpCommand::AuthenticateServer { name: name.to_string() })
            .await
            .map_err(|e| SessionError::CommandChannel(format!("failed to send AuthenticateServer command: {e}")))
    }

    pub(crate) fn mcp_server_statuses(&self) -> Vec<McpServerStatusEntry> {
        self.latest_mcp_snapshot.borrow().server_statuses.clone()
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
        let extra_servers = self
            .mcp_servers
            .iter()
            .map(McpServer::try_clone)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| SessionError::UnsupportedMcpServer(e.to_string()))?;

        let builder = RuntimeBuilder::from_spec(self.cwd.clone(), spec.clone())
            .extra_servers(extra_servers)
            .oauth_handler_factory(mcp_oauth_handler_factory())
            .agent_deps(self.agent_deps.clone());

        let runtime = builder.build(None, Some(initial_messages)).await?;
        let snapshot = McpConnectionDetails {
            catalog: Arc::new(ToolCatalog::new()),
            instructions: BTreeMap::default(),
            tool_definitions: Vec::new(),
            server_statuses: Vec::new(),
        };

        let Runtime { agent_tx, agent_rx, agent_handle, event_rx, mcp_runtime } = runtime;
        Ok(AgentRuntime::new(
            agent,
            agent_tx,
            agent_rx,
            Some(agent_handle),
            event_rx,
            mcp_runtime,
            snapshot,
            runtime_event_tx,
        ))
    }
}

async fn on_mcp_event(
    event: McpClientEvent,
    snapshot_tx: &watch::Sender<McpConnectionDetails>,
    agent_tx: &mpsc::Sender<Command>,
) -> Option<McpClientEvent> {
    match event {
        McpClientEvent::ToolDefinitionsChanged(tool_definitions) => {
            snapshot_tx.send_modify(|snapshot| snapshot.tool_definitions.clone_from(&tool_definitions));
            if let Err(error) = agent_tx.send(Command::agent(AgentCommand::UpdateTools(tool_definitions))).await {
                tracing::error!("Failed to send updated tools to agent runtime: {error:?}");
            }
            None
        }
        McpClientEvent::ServerInstructionsUpdated { server, instructions } => {
            snapshot_tx.send_modify(|snapshot| match &instructions {
                Some(body) => {
                    snapshot.instructions.insert(server.clone(), body.clone());
                }
                None => {
                    snapshot.instructions.remove(&server);
                }
            });
            if let Err(error) =
                agent_tx.send(Command::agent(AgentCommand::UpdateMcpInstructions { server, body: instructions })).await
            {
                tracing::error!("Failed to send updated MCP instructions to agent runtime: {error:?}");
            }
            None
        }
        McpClientEvent::ServerStatusesChanged(server_statuses) => {
            snapshot_tx.send_modify(|snapshot| snapshot.server_statuses.clone_from(&server_statuses));
            Some(McpClientEvent::ServerStatusesChanged(server_statuses))
        }
        McpClientEvent::ConnectionReady(next_snapshot) => {
            snapshot_tx.send_modify(|snapshot| snapshot.clone_from(&next_snapshot));
            tracing::debug!("MCP connection ready");
            Some(McpClientEvent::ConnectionReady(next_snapshot))
        }
        event @ (McpClientEvent::Elicitation(_) | McpClientEvent::AuthenticationFailed { .. }) => Some(event),
    }
}

fn mcp_oauth_handler_factory() -> OAuthHandlerFactory {
    Arc::new(|ctx| {
        ElicitingOAuthHandler::new(ctx)
            .map(|handler| Arc::new(handler) as Arc<dyn OAuthHandler>)
            .map_err(|error| McpError::ConnectionFailed(format!("failed to initialize OAuth handler: {error}")))
    })
}
