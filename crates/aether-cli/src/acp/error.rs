use thiserror::Error;

use crate::error::CliError;
use crate::slash_commands::SlashCommandError;

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("failed to build session: {0}")]
    Build(#[from] CliError),
    #[error("command channel error: {0}")]
    CommandChannel(String),
    #[error("MCP operation failed: {0}")]
    McpOperation(String),
    #[error("unsupported MCP server for agent runtime: {0}")]
    UnsupportedMcpServer(String),
    #[error("agent runtime not found: {0}")]
    AgentNotFound(String),
    #[error("active agent runtime is not running")]
    ActiveRuntimeNotRunning,
}

impl From<SlashCommandError> for SessionError {
    fn from(error: SlashCommandError) -> Self {
        match error {
            SlashCommandError::CommandChannel(message) => Self::CommandChannel(message),
            other => Self::McpOperation(other.to_string()),
        }
    }
}
