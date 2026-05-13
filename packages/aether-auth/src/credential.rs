use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::OAuthError;

/// Credential for an OAuth provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthCredential {
    pub client_id: String,
    pub access_token: String,
    pub refresh_token: Option<String>,
    /// Unix timestamp in milliseconds when the token expires.
    pub expires_at: Option<u64>,
}

/// Trait for loading and saving OAuth credentials, keyed by provider ID or credential key.
///
/// Implementations include [`OsKeyringStore`](crate::OsKeyringStore) (OS keychain, feature `keyring`)
/// and the in-memory [`FakeOAuthCredentialStore`](crate::FakeOAuthCredentialStore) for tests.
#[async_trait]
pub trait OAuthCredentialStorage: Send + Sync {
    async fn load_credential(&self, key: &str) -> Result<Option<OAuthCredential>, OAuthError>;

    async fn save_credential(&self, key: &str, credential: OAuthCredential) -> Result<(), OAuthError>;

    async fn delete_credential(&self, key: &str) -> Result<(), OAuthError>;

    fn has_credential(&self, key: &str) -> bool;
}
