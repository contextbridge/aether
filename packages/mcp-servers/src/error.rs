use thiserror::Error;

#[derive(Debug, Error)]
pub enum ServerInitError {
    #[error("Failed to parse arguments: {0}")]
    InvalidArgs(#[from] clap::Error),
    #[error("{0}")]
    Other(String),
}
