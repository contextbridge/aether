use thiserror::Error;

use crate::error::CliError;

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("failed to build session: {0}")]
    Build(#[from] CliError),
    #[error("command channel error: {0}")]
    CommandChannel(String),
    #[error("MCP operation failed: {0}")]
    McpOperation(String),
    #[error("agent runtime not found: {0}")]
    AgentNotFound(String),
    #[error("active agent runtime is not running")]
    ActiveRuntimeNotRunning,
}
