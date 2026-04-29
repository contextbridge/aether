use super::mappers::map_mcp_prompt_to_available_command;
use aether_core::agent_spec::AgentSpec;
use aether_core::core::AgentHandle;
use aether_core::events::{AgentMessage, UserMessage};
use aether_core::mcp::run_mcp_task::McpCommand;
use llm::ChatMessage;
use mcp_utils::client::oauth::BrowserOAuthHandler;
use mcp_utils::client::{McpClientEvent, McpServerConfig};
use mcp_utils::status::McpServerStatusEntry;

use agent_client_protocol::schema as acp;
use std::collections::HashSet;
use std::path::PathBuf;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::{debug, error};

use crate::runtime::RuntimeBuilder;

/// Represents an active Aether agent session
pub struct Session {
    pub agent_tx: mpsc::Sender<UserMessage>,
    pub agent_rx: mpsc::Receiver<AgentMessage>,
    pub agent_handle: AgentHandle,
    pub _mcp_handle: JoinHandle<()>,
    pub mcp_tx: mpsc::Sender<McpCommand>,
    pub event_rx: mpsc::Receiver<McpClientEvent>,
    pub initial_server_statuses: Vec<McpServerStatusEntry>,
}

impl Session {
    /// Creates a new session with the given LLM provider and configuration.
    ///
    /// Pass `restored_messages` to pre-populate conversation history (e.g. session resume).
    pub async fn new(
        spec: AgentSpec,
        cwd: PathBuf,
        extra_mcp_servers: Vec<McpServerConfig>,
        restored_messages: Option<Vec<ChatMessage>>,
        prompt_cache_key: Option<String>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        debug!("MCP configs: {:?}", spec.mcp_config_sources);
        debug!("Using project root: {:?}", cwd);

        let mut rb = RuntimeBuilder::from_spec(cwd, spec).extra_servers(extra_mcp_servers);

        if let Some(key) = prompt_cache_key {
            rb = rb.prompt_cache_key(key);
        }

        match BrowserOAuthHandler::new() {
            Ok(handler) => {
                rb = rb.oauth_handler(handler);
            }
            Err(e) => {
                error!("Failed to initialize browser OAuth handler: {e}");
            }
        }

        let agent = rb.build(None, restored_messages).await?;

        Ok(Self {
            agent_tx: agent.agent_tx,
            agent_rx: agent.agent_rx,
            agent_handle: agent.agent_handle,
            _mcp_handle: agent.mcp_handle,
            mcp_tx: agent.mcp_tx,
            event_rx: agent.event_rx,
            initial_server_statuses: agent.server_statuses,
        })
    }

    /// Lists available slash commands by querying MCP prompts
    pub async fn list_available_commands(&self) -> Result<Vec<acp::AvailableCommand>, Box<dyn std::error::Error>> {
        let (tx, rx) = oneshot::channel();

        self.mcp_tx
            .send(McpCommand::ListPrompts { tx })
            .await
            .map_err(|e| format!("Failed to send ListPrompts command: {e}"))?;

        let prompts = rx.await.map_err(|e| format!("Failed to receive prompts: {e}"))??;

        let prompt_commands: Vec<_> = prompts.iter().map(map_mcp_prompt_to_available_command).collect();

        Ok(merge_builtin_commands(prompt_commands))
    }
}

fn merge_builtin_commands(commands: Vec<acp::AvailableCommand>) -> Vec<acp::AvailableCommand> {
    let mut merged = builtin_commands();
    let mut seen_names: HashSet<String> = merged.iter().map(|c| c.name.clone()).collect();

    for command in commands {
        if seen_names.insert(command.name.clone()) {
            merged.push(command);
        }
    }

    merged
}

fn builtin_commands() -> Vec<acp::AvailableCommand> {
    vec![]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_builtin_commands_returns_empty_when_no_prompts() {
        let commands = merge_builtin_commands(vec![]);
        assert!(commands.is_empty());
    }

    #[test]
    fn merge_builtin_commands_deduplicates_by_name() {
        let merged = merge_builtin_commands(vec![
            acp::AvailableCommand::new("search", "Search"),
            acp::AvailableCommand::new("search", "Search duplicate"),
        ]);

        let search_count = merged.iter().filter(|c| c.name == "search").count();
        assert_eq!(search_count, 1);
    }
}
