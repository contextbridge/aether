use std::sync::Arc;
use thiserror::Error;

#[derive(Debug, Clone, Error)]
pub enum OAuthError {
    #[error("User cancelled authorization")]
    UserCancelled,

    #[error("OAuth credential storage error: {0}")]
    CredentialStore(String),

    #[error("IO error: {0}")]
    Io(Arc<std::io::Error>),

    #[error("Invalid OAuth callback: {0}")]
    InvalidCallback(String),

    #[error("Token exchange failed: {0}")]
    TokenExchange(String),

    #[error("OAuth state mismatch — possible CSRF attack")]
    StateMismatch,

    #[error("No credentials found: {0}")]
    NoCredentials(String),
}

impl From<std::io::Error> for OAuthError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(Arc::new(error))
    }
}
