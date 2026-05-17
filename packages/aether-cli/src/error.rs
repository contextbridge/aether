use std::io;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CliError {
    #[error("No prompt provided. Pass a prompt as an argument or pipe via stdin.")]
    NoPrompt,
    #[error("{0}")]
    ConflictingArgs(String),
    #[error("Model error: {0}")]
    ModelError(String),
    #[error("MCP error: {0}")]
    McpError(String),
    #[error("IO error: {0}")]
    IoError(#[from] io::Error),
    #[error("Agent error: {0}")]
    AgentError(String),
}
