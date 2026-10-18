#![doc = include_str!("oauth.md")]

mod browser;
mod credential;
pub mod encrypted_file;
pub mod error;
mod memory;

pub use browser::{accept_oauth_callback, authorize_in_browser, bind_loopback};
pub use credential::{OAuthCredential, OAuthCredentialStorage, oauth_http_client};
pub use encrypted_file::EncryptedFileCredentialStore;
pub use error::OAuthError;
pub use memory::MemoryCredentialStore;

#[cfg(feature = "keyring")]
pub mod keyring;
#[cfg(feature = "keyring")]
pub use keyring::KeyringCredentialStore;

#[cfg(feature = "sigv4")]
mod sigv4;
#[cfg(feature = "sigv4")]
pub use sigv4::{SigV4Config, SigV4Credentials, SigV4Error, SigV4Signer};
