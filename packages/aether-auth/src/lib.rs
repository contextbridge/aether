#![doc = include_str!("oauth.md")]

mod browser;
mod credential;
pub mod error;
mod fake;
mod handler;

pub use browser::{BrowserOAuthHandler, accept_oauth_callback, open_browser, wait_for_callback};
pub use credential::{OAuthCredential, OAuthCredentialStorage, oauth_http_client};
pub use error::OAuthError;
pub use fake::FakeOAuthCredentialStore;
pub use handler::{OAuthCallback, OAuthHandler};

#[cfg(feature = "keyring")]
pub mod keyring;
#[cfg(feature = "keyring")]
pub use keyring::OsKeyringStore;

#[cfg(feature = "mcp")]
pub mod mcp;
#[cfg(feature = "mcp")]
pub use mcp::{McpCredentialStore, create_auth_manager_from_store, mcp_credential_store, perform_oauth_flow};
