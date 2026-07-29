use crate::terminal::LifecycleError;
use acp_utils::client::AcpClientError;
use thiserror::Error;

/// Fatal errors that can terminate the experimental TUI.
#[derive(Debug, Error)]
pub enum AppError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Acp(#[from] AcpClientError),
}

impl<E> From<LifecycleError<E>> for AppError
where
    E: Into<AppError>,
{
    fn from(error: LifecycleError<E>) -> Self {
        match error {
            // Init/setup cleanup already restored the terminal; surface the io error.
            LifecycleError::Init(error) | LifecycleError::Setup(error) => AppError::Io(error),
            LifecycleError::Runtime(error) => error.into(),
        }
    }
}
