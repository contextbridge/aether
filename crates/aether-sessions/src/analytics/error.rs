use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum SessionIndexError {
    #[error("could not resolve Aether home; set AETHER_HOME or HOME")]
    MissingAetherHome,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("sqlite error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("sqlite migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error("json error in {path} line {line_number}: {source}")]
    JsonLine { path: PathBuf, line_number: usize, source: serde_json::Error },
    #[error("invalid session metadata in {path}: {message}")]
    InvalidMetadata { path: PathBuf, message: String },
    #[error("query is empty")]
    EmptyQuery,
    #[error("query timed out after {timeout_ms}ms")]
    QueryTimeout { timeout_ms: u64 },
    #[error("background ingest task failed: {0}")]
    BackgroundTask(#[from] tokio::task::JoinError),
    #[error("json serialization error: {0}")]
    Json(#[from] serde_json::Error),
}
