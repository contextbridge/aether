use thiserror::Error;

#[derive(Debug, Error)]
pub enum LspError {
    #[error("Failed to spawn LSP process '{command}': {error}")]
    SpawnFailed { command: String, error: String },

    #[error("LSP initialization failed: {0}")]
    InitializationFailed(String),

    #[error("LSP request '{method}' failed: {error}")]
    RequestFailed { method: String, error: String },

    #[error("Invalid path: {0}")]
    InvalidPath(String),

    #[error("LSP communication channel closed")]
    ChannelClosed,

    #[error("LSP operation timed out")]
    Timeout,
}
