use acp_utils::client::AcpClientError;
use thiserror::Error;

#[doc = include_str!("docs/app_error.md")]
#[derive(Debug, Error)]
pub enum AppError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Acp(#[from] AcpClientError),
}
