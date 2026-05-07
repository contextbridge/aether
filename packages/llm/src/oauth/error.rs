use thiserror::Error;

#[derive(Debug, Error)]
pub enum OAuthError {
    #[error("User cancelled authorization")]
    UserCancelled,

    #[error("OAuth credential storage error: {0}")]
    CredentialStore(String),

    #[error("OS keychain error: {0}")]
    Keychain(#[from] keyring_core::Error),

    #[error("rmcp auth error: {0}")]
    Rmcp(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Invalid OAuth callback: {0}")]
    InvalidCallback(String),

    #[error("Invalid JWT: {0}")]
    InvalidJwt(String),

    #[error("Token exchange failed: {0}")]
    TokenExchange(String),

    #[error("OAuth state mismatch — possible CSRF attack")]
    StateMismatch,

    #[error("No credentials found: {0}")]
    NoCredentials(String),
}
