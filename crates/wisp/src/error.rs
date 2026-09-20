use acp_utils::client::AcpClientError;
use thiserror::Error;

/// Fatal errors that can terminate the TUI.
#[derive(Debug, Error)]
pub enum AppError {
    #[error(
        "ACP connection lost. The agent may still be running. No prompts were retried; reconnect and inspect the session before resubmitting"
    )]
    ConnectionLost,
    #[error("Server did not advertise the Aether remote-server contract; connect to `aether server`")]
    MissingRemoteContract,
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
