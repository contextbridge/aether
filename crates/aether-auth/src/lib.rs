#![doc = include_str!("oauth.md")]

mod browser;
mod credential;
pub mod encrypted_file;
pub mod error;
mod fake;
mod handler;

pub use browser::{BrowserOAuthHandler, accept_oauth_callback, bind_loopback};
pub use credential::{OAuthCredential, OAuthCredentialStorage, oauth_http_client};
pub use encrypted_file::EncryptedFileOAuthCredentialStorage;
pub use error::OAuthError;
pub use fake::FakeOAuthCredentialStore;
pub use handler::OAuthHandler;

#[cfg(feature = "keyring")]
pub mod keyring;
#[cfg(feature = "keyring")]
pub use keyring::OsKeyringStore;

#[cfg(feature = "sigv4")]
mod sigv4;
#[cfg(feature = "sigv4")]
pub use sigv4::{SigV4Config, SigV4Credentials, SigV4Error, SigV4Signer};
