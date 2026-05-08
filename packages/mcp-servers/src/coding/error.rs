//! Error types for coding tools
//!
//! This module provides structured error types for all coding tool operations,
//! replacing the previous `Result<T, String>` pattern with proper `thiserror` enums.

use thiserror::Error;

#[doc = include_str!("../docs/coding_error.md")]
#[derive(Debug, Error)]
pub enum CodingError {
    /// File operation errors (read, write, edit)
    #[error(transparent)]
    File(#[from] FileError),

    /// Bash command execution errors
    #[error(transparent)]
    Bash(#[from] BashError),

    /// Grep search errors
    #[error(transparent)]
    Grep(#[from] GrepError),

    /// Find file errors
    #[error(transparent)]
    Find(#[from] FindError),

    /// List files errors
    #[error(transparent)]
    ListFiles(#[from] ListFilesError),

    /// Web fetch errors
    #[error(transparent)]
    WebFetch(#[from] WebFetchError),

    /// Web search errors
    #[error(transparent)]
    WebSearch(#[from] WebSearchError),

    /// Tool not configured/available
    #[error("{0}")]
    NotConfigured(String),
}

/// Errors related to file operations (read, write, edit)
#[derive(Debug, Error)]
pub enum FileError {
    /// File does not exist
    #[error("File does not exist: {path}")]
    NotFound { path: String },

    /// Failed to read file
    #[error("Failed to read file {path}: {reason}")]
    ReadFailed { path: String, reason: String },

    /// Failed to write file
    #[error("Failed to write to file {path}: {reason}")]
    WriteFailed { path: String, reason: String },

    /// Failed to create parent directories
    #[error("Failed to create directories for {path}: {reason}")]
    CreateDirFailed { path: String, reason: String },

    /// Invalid offset (must be 1-indexed)
    #[error("Invalid offset for file {path}: offset must be 1-indexed (start from 1)")]
    InvalidOffset { path: String },

    /// Invalid edit target line
    #[error("Invalid line for file {path} at edit {edit_index}: {line} ({reason})")]
    InvalidLine { path: String, edit_index: usize, line: usize, reason: String },

    /// Invalid edit request
    #[error("Invalid edit for file {path}{edit_label}: {reason}", edit_label = edit_index.map(|index| format!(" at edit {index}")).unwrap_or_default())]
    InvalidEdit { path: String, edit_index: Option<usize>, reason: String },

    /// Ambiguous overlapping edits
    #[error("Overlapping edits for file {path}: edits {first_edit_index} and {second_edit_index} conflict ({reason})")]
    OverlappingEdits { path: String, first_edit_index: usize, second_edit_index: usize, reason: String },

    /// IO error (wraps `std::io::Error`)
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Errors related to bash command execution
#[derive(Debug, Error)]
pub enum BashError {
    /// Command is forbidden (e.g., rm without flags)
    #[error("{0}")]
    Forbidden(String),

    /// Timeout exceeds maximum allowed
    #[error("Timeout cannot exceed 600000ms (10 minutes)")]
    TimeoutTooLarge,

    /// Failed to spawn process
    #[error("Failed to execute command '{command}': {reason}")]
    SpawnFailed { command: String, reason: String },

    /// Invalid regex pattern for filtering
    #[error("Invalid regex pattern: {0}")]
    InvalidRegex(String),

    /// Failed to join background task
    #[error("Failed to join background task: {0}")]
    JoinFailed(String),

    /// Shell ID not found
    #[error("Shell ID not found: {0}")]
    ShellNotFound(String),

    /// Wait on child process failed
    #[error("Wait failed: {0}")]
    WaitFailed(String),
}

/// Errors related to grep search operations
#[derive(Debug, Error)]
pub enum GrepError {
    /// Invalid glob pattern
    #[error("Invalid glob pattern '{pattern}': {reason}")]
    InvalidGlobPattern { pattern: String, reason: String },

    /// Failed to build glob set
    #[error("Failed to build glob set: {0}")]
    GlobSetBuildFailed(String),

    /// Invalid regex pattern
    #[error("Invalid regex pattern: {0}")]
    InvalidRegex(String),

    /// Search error during file processing
    #[error("Search error: {0}")]
    SearchFailed(String),

    /// Search path does not exist
    #[error("Search path does not exist: {0}")]
    PathNotFound(String),
}

/// Errors related to find file operations
#[derive(Debug, Error)]
pub enum FindError {
    /// Search path does not exist
    #[error("Search path does not exist: {0}")]
    PathNotFound(String),

    /// Invalid glob pattern
    #[error("Invalid glob pattern '{pattern}': {reason}")]
    InvalidGlobPattern { pattern: String, reason: String },

    /// Failed to lock results (mutex poisoned)
    #[error("Failed to lock results")]
    LockFailed,
}

/// Errors related to list files operations
#[derive(Debug, Error)]
pub enum ListFilesError {
    /// Failed to read directory
    #[error("Failed to read directory: {0}")]
    ReadDirFailed(String),

    /// Failed to read directory entry
    #[error("Failed to read entry: {0}")]
    ReadEntryFailed(String),

    /// Failed to read metadata
    #[error("Failed to read metadata: {0}")]
    MetadataFailed(String),
}

/// Errors related to web fetch operations
#[derive(Debug, Error)]
pub enum WebFetchError {
    /// Invalid URL format
    #[error("Invalid URL: {0}")]
    InvalidUrl(String),

    /// HTTP request failed
    #[error("Request failed: {0}")]
    RequestFailed(String),

    /// Request timed out
    #[error("Request timed out after {0}ms")]
    Timeout(u64),

    /// Response too large
    #[error("Response too large: {size} bytes exceeds limit of {limit} bytes")]
    ResponseTooLarge { size: usize, limit: usize },

    /// Failed to parse HTML content
    #[error("Failed to parse HTML: {0}")]
    ParseFailed(String),
}

/// Errors related to web search operations
#[derive(Debug, Error)]
pub enum WebSearchError {
    /// Invalid search query
    #[error("Invalid search query: {0}")]
    InvalidQuery(String),

    /// API request failed
    #[error("API request failed: {0}")]
    ApiError(String),

    /// Rate limit exceeded
    #[error("Rate limit exceeded: {0}")]
    RateLimited(String),

    /// Request timed out
    #[error("Request timed out after {0}ms")]
    Timeout(u64),

    /// Configuration error
    #[error("Configuration error: {0}")]
    ConfigError(String),

    /// Failed to parse API response
    #[error("Failed to parse API response: {0}")]
    ParseError(String),
}
