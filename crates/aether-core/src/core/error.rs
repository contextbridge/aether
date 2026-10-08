use thiserror::Error;

#[derive(Debug, Error)]
pub enum AgentError {
    #[error(transparent)]
    McpSpawn(#[from] crate::mcp::McpSpawnError),
    /// LLM provider error
    #[error("LLM error: {0}")]
    LlmError(#[from] llm::LlmError),
    /// IO error (file operations, etc.)
    #[error("IO error: {0}")]
    IoError(String),
    /// Generic error for other cases
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, AgentError>;
