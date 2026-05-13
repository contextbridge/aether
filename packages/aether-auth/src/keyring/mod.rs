//! OS-keychain-backed implementation of the [`OAuthCredentialStorage`] trait.

use async_trait::async_trait;
use keyring_core::{CredentialStore as KeyringCredentialStore, Entry, Error as KeyringError};
use std::sync::{Arc, LazyLock};
use tokio::task;

use crate::{OAuthCredential, OAuthCredentialStorage, OAuthError};

const KEYCHAIN_SERVICE: &str = "aether-oauth-v1";

/// OAuth credential store backed by the OS keychain (Apple Keychain on macOS,
/// Credential Manager on Windows, Secret Service over D-Bus on Linux/FreeBSD).
#[derive(Clone)]
pub struct OsKeyringStore {
    inner: Arc<LazyLock<Result<Arc<KeyringCredentialStore>, OAuthError>, BackendFactory>>,
}

impl OsKeyringStore {
    pub fn new(keyring_store: Arc<KeyringCredentialStore>) -> Self {
        Self::from_factory(Box::new(move || Ok(keyring_store)))
    }

    /// Build a store backed by the platform's native keychain.
    pub fn with_platform_store() -> Self {
        Self::from_factory(Box::new(create_platform_keyring_store))
    }

    /// Build a store backed by an in-memory mock keyring (for tests that exercise
    /// the rmcp `CredentialStore` adapter without needing the real OS keychain).
    pub fn with_mock_store() -> Result<Self, OAuthError> {
        Ok(Self::new(keyring_core::mock::Store::new().map_err(map_keyring_err)?))
    }

    fn from_factory(factory: BackendFactory) -> Self {
        Self { inner: Arc::new(LazyLock::new(factory)) }
    }

    fn resolve_store(&self) -> Result<Arc<KeyringCredentialStore>, OAuthError> {
        match &**self.inner {
            Ok(store) => Ok(Arc::clone(store)),
            Err(e) => Err(e.clone()),
        }
    }
}

#[async_trait]
impl OAuthCredentialStorage for OsKeyringStore {
    async fn load_credential(&self, key: &str) -> Result<Option<OAuthCredential>, OAuthError> {
        let store = self.clone();
        let key = key.to_string();
        spawn_blocking(move || load_from_keyring(&store, &key)).await
    }

    async fn save_credential(&self, key: &str, credential: OAuthCredential) -> Result<(), OAuthError> {
        let store = self.clone();
        let key = key.to_string();
        spawn_blocking(move || save_to_keyring(&store, &key, &credential)).await
    }

    async fn delete_credential(&self, key: &str) -> Result<(), OAuthError> {
        let store = self.clone();
        let key = key.to_string();
        spawn_blocking(move || delete_from_keyring(&store, &key)).await
    }

    fn has_credential(&self, key: &str) -> bool {
        try_has_credential(self, key).unwrap_or(false)
    }
}

type BackendFactory = Box<dyn FnOnce() -> Result<Arc<KeyringCredentialStore>, OAuthError> + Send + Sync>;

fn try_has_credential(store: &OsKeyringStore, key: &str) -> Result<bool, OAuthError> {
    let entry = credential_entry(store, key)?;
    match entry.get_credential() {
        Ok(_) => Ok(true),
        Err(KeyringError::NoEntry) => Ok(false),
        Err(err) => Err(map_keyring_err(err)),
    }
}

fn credential_entry(store: &OsKeyringStore, key: &str) -> Result<Entry, OAuthError> {
    let backend = store.resolve_store()?;
    build_keyring_entry(backend.as_ref(), key)
}

fn load_from_keyring(store: &OsKeyringStore, key: &str) -> Result<Option<OAuthCredential>, OAuthError> {
    let entry = credential_entry(store, key)?;
    match entry.get_secret() {
        Ok(blob) => serde_json::from_slice(&blob)
            .map(Some)
            .map_err(|e| OAuthError::CredentialStore(format!("invalid credential: {e}"))),
        Err(KeyringError::NoEntry) => Ok(None),
        Err(err) => Err(map_keyring_err(err)),
    }
}

fn save_to_keyring(store: &OsKeyringStore, key: &str, credential: &OAuthCredential) -> Result<(), OAuthError> {
    let entry = credential_entry(store, key)?;
    let blob = serde_json::to_vec(credential)
        .map_err(|e| OAuthError::CredentialStore(format!("failed to serialize credential: {e}")))?;
    entry.set_secret(&blob).map_err(map_keyring_err)?;
    Ok(())
}

fn delete_from_keyring(store: &OsKeyringStore, key: &str) -> Result<(), OAuthError> {
    let entry = credential_entry(store, key)?;
    match entry.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
        Err(err) => Err(map_keyring_err(err)),
    }
}

#[cfg(target_os = "macos")]
fn create_platform_keyring_store() -> Result<Arc<KeyringCredentialStore>, OAuthError> {
    let store: Arc<KeyringCredentialStore> =
        apple_native_keyring_store::keychain::Store::new().map_err(map_keyring_err)?;
    Ok(store)
}

#[cfg(target_os = "windows")]
fn create_platform_keyring_store() -> Result<Arc<KeyringCredentialStore>, OAuthError> {
    let store: Arc<KeyringCredentialStore> = windows_native_keyring_store::Store::new().map_err(map_keyring_err)?;
    Ok(store)
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn create_platform_keyring_store() -> Result<Arc<KeyringCredentialStore>, OAuthError> {
    let store: Arc<KeyringCredentialStore> =
        dbus_secret_service_keyring_store::Store::new().map_err(map_keyring_err)?;
    Ok(store)
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux", target_os = "freebsd")))]
fn create_platform_keyring_store() -> Result<Arc<KeyringCredentialStore>, OAuthError> {
    Err(OAuthError::CredentialStore("OS keychain is not supported on this platform".to_string()))
}

#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
fn build_keyring_entry(store: &KeyringCredentialStore, key: &str) -> Result<Entry, OAuthError> {
    store.build(KEYCHAIN_SERVICE, key, None).map_err(map_keyring_err)
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn build_keyring_entry(store: &KeyringCredentialStore, key: &str) -> Result<Entry, OAuthError> {
    if store.as_any().is::<keyring_core::mock::Store>() {
        return store.build(KEYCHAIN_SERVICE, key, None).map_err(map_keyring_err);
    }

    let label = format!("Aether OAuth: {key}");
    let modifiers = std::collections::HashMap::from([("label", label.as_str())]);
    store.build(KEYCHAIN_SERVICE, key, Some(&modifiers)).map_err(map_keyring_err)
}

#[allow(clippy::needless_pass_by_value)]
fn map_keyring_err(err: KeyringError) -> OAuthError {
    OAuthError::CredentialStore(format!("OS keychain error: {err}"))
}

async fn spawn_blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, OAuthError> + Send + 'static,
) -> Result<T, OAuthError> {
    task::spawn_blocking(f).await.map_err(|e| OAuthError::CredentialStore(format!("credential task failed: {e}")))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn credential() -> OAuthCredential {
        OAuthCredential {
            client_id: "client".to_string(),
            access_token: "access".to_string(),
            refresh_token: Some("refresh".to_string()),
            expires_at: Some(1234),
        }
    }

    #[tokio::test]
    async fn load_returns_none_when_missing() {
        let store = OsKeyringStore::with_mock_store().unwrap();
        assert!(store.load_credential("server").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn save_then_load_round_trips() {
        let store = OsKeyringStore::with_mock_store().unwrap();
        store.save_credential("server", credential()).await.unwrap();

        let loaded = store.load_credential("server").await.unwrap().unwrap();
        assert_eq!(loaded.client_id, "client");
        assert_eq!(loaded.access_token, "access");
        assert_eq!(loaded.refresh_token.as_deref(), Some("refresh"));
        assert_eq!(loaded.expires_at, Some(1234));
    }

    #[tokio::test]
    async fn credential_keys_are_isolated() {
        let store = OsKeyringStore::with_mock_store().unwrap();
        store.save_credential("key-a", credential()).await.unwrap();
        assert!(store.load_credential("key-b").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn delete_removes_credential() {
        let store = OsKeyringStore::with_mock_store().unwrap();
        store.save_credential("server", credential()).await.unwrap();
        assert!(store.has_credential("server"));

        store.delete_credential("server").await.unwrap();
        assert!(!store.has_credential("server"));
    }

    #[tokio::test]
    async fn load_reports_invalid_json() {
        use keyring_core::api::CredentialStoreApi;

        let mock = keyring_core::mock::Store::new().unwrap();
        let entry = mock.build(KEYCHAIN_SERVICE, "server", None).unwrap();
        entry.set_secret(b"not-json").unwrap();
        let store = OsKeyringStore::new(mock);

        let err = store.load_credential("server").await.unwrap_err();
        assert!(matches!(err, OAuthError::CredentialStore(m) if m.contains("invalid credential")));
    }

    #[tokio::test]
    async fn operations_return_error_when_backend_construction_fails() {
        let store = OsKeyringStore::from_factory(Box::new(|| Err(OAuthError::CredentialStore("no dbus".to_string()))));

        let load_err = store.load_credential("k").await.unwrap_err();
        assert!(matches!(load_err, OAuthError::CredentialStore(m) if m.contains("no dbus")));

        let save_err = store.save_credential("k", credential()).await.unwrap_err();
        assert!(matches!(save_err, OAuthError::CredentialStore(m) if m.contains("no dbus")));

        let delete_err = store.delete_credential("k").await.unwrap_err();
        assert!(matches!(delete_err, OAuthError::CredentialStore(m) if m.contains("no dbus")));

        assert!(!store.has_credential("k"));
    }

    #[tokio::test]
    async fn backend_construction_failure_is_cached() {
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = Arc::clone(&counter);
        let store = OsKeyringStore::from_factory(Box::new(move || {
            counter_clone.fetch_add(1, Ordering::SeqCst);
            Err(OAuthError::CredentialStore("no dbus".to_string()))
        }));

        let _ = store.load_credential("k").await;
        let _ = store.load_credential("k").await;

        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }
}
