#[derive(Debug, thiserror::Error)]
pub enum SessionLogError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("missing session metadata line")]
    MissingMetadata,
    #[error("invalid session metadata on line {line_number}: {source}")]
    InvalidMetadata { line_number: usize, source: serde_json::Error },
}

#[derive(Debug, thiserror::Error)]
pub enum SessionStoreError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("Aether home directory is not configured")]
    MissingAetherHome,
    #[error("missing session metadata line")]
    MissingMetadata,
    #[error("invalid session metadata on line {line_number}: {source}")]
    InvalidMetadata { line_number: usize, source: serde_json::Error },
    #[error("failed to serialize session data: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("prompt history index failed: {0}")]
    PromptHistory(#[source] std::io::Error),
}
