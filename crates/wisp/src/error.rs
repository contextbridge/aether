use acp_utils::client::AcpClientError;
use thiserror::Error;

/// Fatal errors that can terminate the TUI.
#[derive(Debug, Error)]
pub enum AppError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Acp(#[from] AcpClientError),
    #[error(transparent)]
    Render(#[from] RenderError<std::io::Error>),
}

/// Failures while rendering a frame or committing rows to native scrollback.
#[derive(Debug, Error)]
pub enum RenderError<E> {
    #[error("terminal rendering failed: {0}")]
    Backend(#[source] E),
    #[error("streaming markdown layout failed: {0}")]
    MarkdownStream(#[from] clankerdiff_ratatui::MarkdownStreamError),
    #[error("streaming markdown history commit failed: {0}")]
    MarkdownCommit(#[from] clankerdiff_ratatui::MarkdownCommitError),
}
