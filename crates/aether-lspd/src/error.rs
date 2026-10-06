use std::io;
use std::time::Duration;
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

    /// The LSP process exited or rejected `initialize` before completing the handshake
    #[error("Language server failed to complete initialization (it exited or rejected `initialize`)")]
    LspInitializeFailed,

    /// The LSP process never answered `initialize`
    #[error("Language server did not answer `initialize` within {}s", .0.as_secs())]
    LspInitializeTimedOut(Duration),

    /// Lockfile error
    #[error("Lockfile error: {0}")]
    LockfileError(#[source] io::Error),
}

/// Result type for daemon operations
pub type DaemonResult<T> = std::result::Result<T, DaemonError>;
