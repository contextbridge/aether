#![doc = include_str!("../docs/oauth.md")]

pub mod browser;
pub mod credential_store;
pub mod error;
pub mod handler;
pub mod integration;

pub use browser::{BrowserOAuthHandler, accept_oauth_callback};
pub use credential_store::{McpCredentialStore, OAuthCredential, OAuthCredentialStorage, OAuthCredentialStore};
pub use error::OAuthError;
pub use handler::{OAuthCallback, OAuthHandler};
pub use integration::{create_auth_manager_from_store, perform_oauth_flow};
