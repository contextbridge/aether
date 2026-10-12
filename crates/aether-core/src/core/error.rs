use thiserror::Error;

#[derive(Debug, Error)]
pub enum AgentError {
    /// LLM provider error
    #[error("LLM error: {0}")]
    LlmError(#[from] llm::LlmError),
    /// IO error (file operations, etc.)
    #[error("IO error: {0}")]
    IoError(String),
}

pub type Result<T> = std::result::Result<T, AgentError>;
