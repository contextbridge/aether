use thiserror::Error;

use crate::error::CliError;

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("failed to build session: {0}")]
    Build(#[from] CliError),
    #[error("failed to persist session: {0}")]
    Persistence(#[from] aether_sessions::SessionStoreError),
    #[error("agent command channel is closed")]
    CommandChannelClosed,
    #[error("{0}")]
    TurnFailed(String),
    #[error("MCP operation failed: {0}")]
    McpOperation(#[from] aether_core::mcp::McpHandleError),
    #[error("model configuration failed: {0}")]
    Model(#[from] llm::LlmError),
    #[cfg(any(test, feature = "testing"))]
    #[error("MCP runtime stopped during startup")]
    McpStartupStopped,
    #[error("agent runtime not found: {0}")]
    AgentNotFound(String),
    #[error("session shut down during runtime startup")]
    Cancelled,
    #[error("active agent runtime is not running")]
    ActiveRuntimeNotRunning,
}
