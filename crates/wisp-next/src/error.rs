use acp_utils::client::AcpClientError;
use thiserror::Error;

/// Fatal errors that can terminate the experimental TUI.
#[derive(Debug, Error)]
pub enum AppError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Acp(#[from] AcpClientError),
    #[error("{0}")]
    Startup(#[from] wisp::error::AppError),
}
