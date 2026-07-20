//! LSP-specific error types

use aether_lspd::ClientError;
use thiserror::Error;

#[doc = include_str!("../docs/lsp_error.md")]
#[derive(Debug, Error)]
pub enum LspError {
    /// I/O error (e.g., reading a file)
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// LSP client / daemon communication error
    #[error(transparent)]
    Client(#[from] ClientError),

    /// The language server is configured but unavailable.
    #[error("Language server unavailable: {0}")]
    ServerUnavailable(String),

    /// No language server is configured for the requested language.
    #[error("No LSP configured for {0}")]
    UnsupportedLanguage(String),

    /// The requested path cannot be represented as a file URI.
    #[error("Invalid path: {0}")]
    InvalidPath(String),

    /// The requested symbol could not be found.
    #[error("Symbol not found: {0}")]
    SymbolNotFound(String),

    /// The requested source position is invalid.
    #[error("Invalid source position: {0}")]
    InvalidPosition(String),

    #[error("Transport error: {0}")]
    Transport(String),
}

/// Result type alias for LSP operations
pub type Result<T> = std::result::Result<T, LspError>;
