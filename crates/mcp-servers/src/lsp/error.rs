//! LSP-specific error types

use aether_lspd::{ClientError, UriError};
use thiserror::Error;

use crate::search::SearchError;

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

    /// The requested query was empty or otherwise invalid.
    #[error("Invalid query: {0}")]
    InvalidQuery(String),

    /// An LSP edit produced an invalid or unsupported change.
    #[error("Invalid edit: {0}")]
    InvalidEdit(String),

    /// A backing file search failed.
    #[error("Search error: {0}")]
    Search(#[from] SearchError),

    #[error("Transport error: {0}")]
    Transport(String),
}

impl From<UriError> for LspError {
    fn from(error: UriError) -> Self {
        Self::InvalidPath(error.to_string())
    }
}

/// Result type alias for LSP operations
pub type Result<T> = std::result::Result<T, LspError>;
