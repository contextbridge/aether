use thiserror::Error;

use crate::runtime::RuntimeError;

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("failed to build session: {0}")]
    Build(#[from] RuntimeError),
    #[error("failed to persist session: {0}")]
    Persistence(#[from] aether_sessions::SessionStoreError),
    #[error("agent command channel is closed")]
    CommandChannelClosed,
    #[error("model configuration failed: {0}")]
    Model(#[from] llm::LlmError),
    #[error("agent runtime not found: {0}")]
    AgentNotFound(String),
    #[error("session shut down during runtime startup")]
    Cancelled,
}
