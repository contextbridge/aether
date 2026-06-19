use std::io;
use thiserror::Error;

#[doc = include_str!("docs/daemon_error.md")]
#[derive(Debug, Error)]
pub enum DaemonError {
    /// IO error
    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    /// Failed to bind to socket
    #[error("Failed to bind to socket: {0}")]
    BindFailed(#[source] io::Error),

    /// Failed to spawn LSP process
    #[error("Failed to spawn LSP: {0}")]
    LspSpawnFailed(String),

    /// Lockfile error
    #[error("Lockfile error: {0}")]
    LockfileError(String),
}

/// Result type for daemon operations
pub type DaemonResult<T> = std::result::Result<T, DaemonError>;
