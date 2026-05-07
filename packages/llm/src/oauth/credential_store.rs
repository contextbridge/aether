use async_trait::async_trait;
use keyring_core::{CredentialStore as KeyringCredentialStore, Entry, Error as KeyringError};
use oauth2::{AccessToken, RefreshToken, TokenResponse};
use rmcp::transport::auth::{
    AuthError, CredentialStore, OAuthTokenResponse, StoredCredentials, VendorExtraTokenFields,
};
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use super::OAuthError;

const KEYCHAIN_SERVICE: &str = "aether-oauth-v1";

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
/// The default implementation (`OAuthCredentialStore`) uses the OS keychain.
/// Tests can use an in-memory fake to avoid keychain popups.
pub trait OAuthCredentialStorage: Send + Sync {
    fn load_credential(&self, key: &str) -> impl Future<Output = Result<Option<OAuthCredential>, OAuthError>> + Send;

    fn save_credential(
        &self,
        key: &str,
        credential: OAuthCredential,
    ) -> impl Future<Output = Result<(), OAuthError>> + Send;

    fn has_credential(&self, key: &str) -> bool;
}

/// Shared OAuth credential repository backed by a keyring-core credential store.
///
/// Each provider/server key maps to its own keychain entry.
#[derive(Clone)]
pub struct OAuthCredentialStore {
    keyring_store: Arc<KeyringCredentialStore>,
}

/// Per-server adapter for rmcp OAuth credential storage.
#[derive(Clone)]
pub struct McpCredentialStore {
    server_id: String,
    store: OAuthCredentialStore,
}

impl OAuthCredentialStore {
    pub fn new(keyring_store: Arc<KeyringCredentialStore>) -> Self {
        Self { keyring_store }
    }

    pub fn with_platform_store() -> Result<Self, OAuthError> {
        Ok(Self::new(create_platform_keyring_store()?))
    }

    pub fn with_mock_store() -> Result<Self, OAuthError> {
        Ok(Self::new(keyring_core::mock::Store::new()?))
    }

    pub fn mcp_store(&self, server_id: &str) -> McpCredentialStore {
        McpCredentialStore { server_id: server_id.to_string(), store: self.clone() }
    }

    pub async fn load_credential(&self, key: &str) -> Result<Option<OAuthCredential>, OAuthError> {
        let store = self.clone();
        let key = key.to_string();
        spawn_blocking(move || store.load_from_keyring(&key)).await
    }

    pub async fn save_credential(&self, key: &str, credential: OAuthCredential) -> Result<(), OAuthError> {
        let store = self.clone();
        let key = key.to_string();
        spawn_blocking(move || store.save_to_keyring(&key, &credential)).await
    }

    pub async fn delete_credential(&self, key: &str) -> Result<(), OAuthError> {
        let store = self.clone();
        let key = key.to_string();
        spawn_blocking(move || store.delete_from_keyring(&key)).await
    }

    pub fn try_has_credential(&self, key: &str) -> Result<bool, OAuthError> {
        let entry = self.credential_entry(key)?;
        match entry.get_credential() {
            Ok(_) => Ok(true),
            Err(KeyringError::NoEntry) => Ok(false),
            Err(err) => Err(err.into()),
        }
    }

    pub fn has_credential(&self, key: &str) -> bool {
        self.try_has_credential(key).unwrap_or(false)
    }

    fn credential_entry(&self, key: &str) -> Result<Entry, OAuthError> {
        build_keyring_entry(self.keyring_store.as_ref(), key)
    }

    fn load_from_keyring(&self, key: &str) -> Result<Option<OAuthCredential>, OAuthError> {
        let entry = self.credential_entry(key)?;
        match entry.get_secret() {
            Ok(blob) => serde_json::from_slice(&blob)
                .map(Some)
                .map_err(|err| OAuthError::CredentialStore(format!("invalid credential: {err}"))),
            Err(KeyringError::NoEntry) => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    fn save_to_keyring(&self, key: &str, credential: &OAuthCredential) -> Result<(), OAuthError> {
        let entry = self.credential_entry(key)?;
        let blob = serde_json::to_vec(credential)
            .map_err(|err| OAuthError::CredentialStore(format!("failed to serialize credential: {err}")))?;
        entry.set_secret(&blob)?;
        Ok(())
    }

    fn delete_from_keyring(&self, key: &str) -> Result<(), OAuthError> {
        let entry = self.credential_entry(key)?;
        match entry.delete_credential() {
            Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
            Err(err) => Err(err.into()),
        }
    }
}

impl OAuthCredentialStorage for OAuthCredentialStore {
    async fn load_credential(&self, key: &str) -> Result<Option<OAuthCredential>, OAuthError> {
        OAuthCredentialStore::load_credential(self, key).await
    }

    async fn save_credential(&self, key: &str, credential: OAuthCredential) -> Result<(), OAuthError> {
        OAuthCredentialStore::save_credential(self, key, credential).await
    }

    fn has_credential(&self, key: &str) -> bool {
        OAuthCredentialStore::has_credential(self, key)
    }
}

#[async_trait]
impl CredentialStore for McpCredentialStore {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        let cred =
            self.store.load_credential(&self.server_id).await.map_err(|e| AuthError::InternalError(e.to_string()))?;

        Ok(cred.map(|c| {
            let token_response = build_token_response(&c);
            build_stored_credentials(&c.client_id, Some(&token_response))
        }))
    }

    async fn save(&self, credentials: StoredCredentials) -> Result<(), AuthError> {
        let token = credentials
            .token_response
            .ok_or_else(|| AuthError::InternalError("No token response to save".to_string()))?;

        let expires_at = token.expires_in().map(|duration| {
            let now_ms = u64::try_from(
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis(),
            )
            .unwrap_or(u64::MAX);
            let duration_ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
            now_ms.saturating_add(duration_ms)
        });

        let credential = OAuthCredential {
            client_id: credentials.client_id,
            access_token: token.access_token().secret().clone(),
            refresh_token: token.refresh_token().map(|t| t.secret().clone()),
            expires_at,
        };

        self.store
            .save_credential(&self.server_id, credential)
            .await
            .map_err(|e| AuthError::InternalError(e.to_string()))
    }

    async fn clear(&self) -> Result<(), AuthError> {
        self.store.delete_credential(&self.server_id).await.map_err(|e| AuthError::InternalError(e.to_string()))
    }
}

#[cfg(target_os = "macos")]
fn create_platform_keyring_store() -> Result<Arc<KeyringCredentialStore>, OAuthError> {
    let store: Arc<KeyringCredentialStore> = apple_native_keyring_store::keychain::Store::new()?;
    Ok(store)
}

#[cfg(target_os = "windows")]
fn create_platform_keyring_store() -> Result<Arc<KeyringCredentialStore>, OAuthError> {
    let store: Arc<KeyringCredentialStore> = windows_native_keyring_store::Store::new()?;
    Ok(store)
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn create_platform_keyring_store() -> Result<Arc<KeyringCredentialStore>, OAuthError> {
    let store: Arc<KeyringCredentialStore> = dbus_secret_service_keyring_store::Store::new()?;
    Ok(store)
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux", target_os = "freebsd")))]
fn create_platform_keyring_store() -> Result<Arc<KeyringCredentialStore>, OAuthError> {
    Err(OAuthError::CredentialStore("OS keychain is not supported on this platform".to_string()))
}

#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
fn build_keyring_entry(store: &KeyringCredentialStore, key: &str) -> Result<Entry, OAuthError> {
    Ok(store.build(KEYCHAIN_SERVICE, key, None)?)
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn build_keyring_entry(store: &KeyringCredentialStore, key: &str) -> Result<Entry, OAuthError> {
    if store.as_any().is::<keyring_core::mock::Store>() {
        return Ok(store.build(KEYCHAIN_SERVICE, key, None)?);
    }

    let label = format!("Aether OAuth: {key}");
    let modifiers = std::collections::HashMap::from([("label", label.as_str())]);
    Ok(store.build(KEYCHAIN_SERVICE, key, Some(&modifiers))?)
}

async fn spawn_blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, OAuthError> + Send + 'static,
) -> Result<T, OAuthError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|err| OAuthError::CredentialStore(format!("credential task failed: {err}")))?
}

/// Construct a `StoredCredentials` via serde deserialization.
///
/// The upstream struct is `#[non_exhaustive]` with no constructor, so this is
/// the only way to build one from outside the crate.
fn build_stored_credentials(client_id: &str, token_response: Option<&OAuthTokenResponse>) -> StoredCredentials {
    serde_json::from_value(serde_json::json!({
        "client_id": client_id,
        "token_response": token_response,
    }))
    .expect("StoredCredentials deserialization from known-good fields cannot fail")
}

fn build_token_response(cred: &OAuthCredential) -> OAuthTokenResponse {
    let mut response = OAuthTokenResponse::new(
        AccessToken::new(cred.access_token.clone()),
        oauth2::basic::BasicTokenType::Bearer,
        VendorExtraTokenFields::default(),
    );

    if let Some(ref refresh) = cred.refresh_token {
        response.set_refresh_token(Some(RefreshToken::new(refresh.clone())));
    }

    if let Some(expires_at_millis) = cred.expires_at {
        let now_millis = u64::try_from(
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis(),
        )
        .unwrap_or(u64::MAX);

        if expires_at_millis > now_millis {
            response.set_expires_in(Some(&Duration::from_millis(expires_at_millis - now_millis)));
        }
    }

    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn load_returns_none_when_missing() {
        let store = test_store();
        assert!(store.load_credential("server").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn save_then_load_round_trips_secret_bytes() {
        let store = test_store();
        store.save_credential("server", credential()).await.unwrap();

        let loaded = store.load_credential("server").await.unwrap().unwrap();
        assert_eq!(loaded.client_id, "client");
        assert_eq!(loaded.access_token, "access");
        assert_eq!(loaded.refresh_token.as_deref(), Some("refresh"));
        assert_eq!(loaded.expires_at, Some(1234));
    }

    #[tokio::test]
    async fn credential_keys_are_isolated() {
        let store = test_store();
        store.save_credential("key-a", credential()).await.unwrap();

        assert!(store.load_credential("key-b").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn clear_removes_saved_credential_through_mcp_store() {
        let store = test_store();
        store.save_credential("server", credential()).await.unwrap();

        let mcp_store = store.mcp_store("server");
        CredentialStore::clear(&mcp_store).await.unwrap();

        assert!(store.load_credential("server").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn has_credential_reflects_keyring_state() {
        let store = test_store();
        assert!(!store.has_credential("server"));

        store.save_credential("server", credential()).await.unwrap();
        assert!(store.has_credential("server"));

        CredentialStore::clear(&store.mcp_store("server")).await.unwrap();
        assert!(!store.has_credential("server"));
    }

    #[tokio::test]
    async fn load_reports_invalid_json() {
        let keyring_store = mock_keyring_store();
        let store = OAuthCredentialStore::new(keyring_store.clone());
        let entry = keyring_store.build(KEYCHAIN_SERVICE, "server", None).unwrap();
        entry.set_secret(b"not-json").unwrap();

        let err = store.load_credential("server").await.unwrap_err();
        assert!(matches!(err, OAuthError::CredentialStore(message) if message.contains("invalid credential")));
    }

    #[tokio::test]
    async fn mcp_store_round_trips_stored_credentials() {
        let store = test_store();
        let mcp_store = store.mcp_store("server");
        let credential = credential();
        let token_response = build_token_response(&credential);
        let stored = build_stored_credentials(&credential.client_id, Some(&token_response));

        CredentialStore::save(&mcp_store, stored).await.unwrap();

        let loaded = CredentialStore::load(&mcp_store).await.unwrap().unwrap();
        let token = loaded.token_response.unwrap();
        assert_eq!(loaded.client_id, "client");
        assert_eq!(token.access_token().secret(), "access");
        assert_eq!(token.refresh_token().map(|t| t.secret().as_str()), Some("refresh"));
    }

    fn mock_keyring_store() -> Arc<keyring_core::CredentialStore> {
        keyring_core::mock::Store::new().unwrap()
    }

    fn test_store() -> OAuthCredentialStore {
        OAuthCredentialStore::new(mock_keyring_store())
    }

    fn credential() -> OAuthCredential {
        OAuthCredential {
            client_id: "client".to_string(),
            access_token: "access".to_string(),
            refresh_token: Some("refresh".to_string()),
            expires_at: Some(1234),
        }
    }
}
