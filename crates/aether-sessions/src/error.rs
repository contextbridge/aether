#[derive(Debug, thiserror::Error)]
pub enum SessionLogError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("missing session metadata line")]
    MissingMetadata,
    #[error("invalid session metadata on line {line_number}: {source}")]
    InvalidMetadata { line_number: usize, source: serde_json::Error },
}
