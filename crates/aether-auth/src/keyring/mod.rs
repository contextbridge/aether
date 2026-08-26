//! OS-keychain-backed implementation of the [`OAuthCredentialStorage`] trait.

use async_trait::async_trait;
use keyring_core::{CredentialStore as KeyringCredentialStore, Entry, Error as KeyringError};
use serde_json::Value;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::task;

use crate::{OAuthCredentialStorage, OAuthError};

const KEYCHAIN_SERVICE: &str = "aether-oauth-v1";

/// OAuth credential store backed by the OS keychain (Apple Keychain on macOS,
/// Credential Manager on Windows, Secret Service over D-Bus on Linux/FreeBSD).
#[derive(Clone)]
pub struct OsKeyringStore {
    inner: Arc<KeyringBackend>,
}

impl OsKeyringStore {
    pub fn new(keyring_store: Arc<KeyringCredentialStore>) -> Self {
        Self::from_factory(Arc::new(move || Ok(Arc::clone(&keyring_store))))
    }

    /// Build a store backed by the platform's native keychain.
    pub fn with_platform_store() -> Self {
        Self::from_factory(Arc::new(create_platform_keyring_store))
    }

    /// Build a store backed by an in-memory mock keyring (for tests that exercise
    /// the rmcp `CredentialStore` adapter without needing the real OS keychain).
    pub fn with_mock_store() -> Result<Self, OAuthError> {
        Ok(Self::new(keyring_core::mock::Store::new().map_err(map_keyring_err)?))
    }

    fn from_factory(factory: BackendFactory) -> Self {
        Self { inner: Arc::new(KeyringBackend { factory, store: Mutex::new(None) }) }
    }

    fn resolve_store(&self) -> Result<Arc<KeyringCredentialStore>, OAuthError> {
        let mut store = self.inner.store.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(store) = store.as_ref() {
            return Ok(Arc::clone(store));
        }

        let created = (self.inner.factory)()?;
        *store = Some(Arc::clone(&created));
        Ok(created)
    }

    fn invalidate_store(&self, failed: &Arc<KeyringCredentialStore>) {
        let mut store = self.inner.store.lock().unwrap_or_else(PoisonError::into_inner);
        if store.as_ref().is_some_and(|current| Arc::ptr_eq(current, failed)) {
            *store = None;
        }
    }
}

#[async_trait]
impl OAuthCredentialStorage for OsKeyringStore {
    async fn load(&self, key: &str) -> Result<Option<Value>, OAuthError> {
        let store = self.clone();
        let key = key.to_string();
        spawn_blocking(move || load_from_keyring(&store, &key)).await
    }

    async fn save(&self, key: &str, value: Value) -> Result<(), OAuthError> {
        let store = self.clone();
        let key = key.to_string();
        spawn_blocking(move || save_to_keyring(&store, &key, &value)).await
    }

    async fn delete(&self, key: &str) -> Result<(), OAuthError> {
        let store = self.clone();
        let key = key.to_string();
        spawn_blocking(move || delete_from_keyring(&store, &key)).await
    }

    fn contains(&self, key: &str) -> bool {
        try_contains(self, key).unwrap_or(false)
    }
}

struct KeyringBackend {
    factory: BackendFactory,
    store: Mutex<Option<Arc<KeyringCredentialStore>>>,
}

type BackendFactory = Arc<dyn Fn() -> Result<Arc<KeyringCredentialStore>, OAuthError> + Send + Sync>;

fn try_contains(store: &OsKeyringStore, key: &str) -> Result<bool, OAuthError> {
    with_keyring_entry(store, key, |entry| match entry.get_credential() {
        Ok(_) => Ok(true),
        Err(KeyringError::NoEntry) => Ok(false),
        Err(err) => Err(err),
    })
}

fn load_from_keyring(store: &OsKeyringStore, key: &str) -> Result<Option<Value>, OAuthError> {
    let blob = with_keyring_entry(store, key, |entry| match entry.get_secret() {
        Ok(blob) => Ok(Some(blob)),
        Err(KeyringError::NoEntry) => Ok(None),
        Err(err) => Err(err),
    })?;
    blob.map(|blob| {
        serde_json::from_slice(&blob).map_err(|e| OAuthError::CredentialStore(format!("invalid credential: {e}")))
    })
    .transpose()
}

fn save_to_keyring(store: &OsKeyringStore, key: &str, value: &Value) -> Result<(), OAuthError> {
    let blob = serde_json::to_vec(value)
        .map_err(|e| OAuthError::CredentialStore(format!("failed to serialize credential: {e}")))?;
    with_keyring_entry(store, key, |entry| entry.set_secret(&blob))
}

fn delete_from_keyring(store: &OsKeyringStore, key: &str) -> Result<(), OAuthError> {
    with_keyring_entry(store, key, |entry| match entry.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
        Err(err) => Err(err),
    })
}

fn with_keyring_entry<T>(
    store: &OsKeyringStore,
    key: &str,
    operation: impl Fn(&Entry) -> Result<T, KeyringError>,
) -> Result<T, OAuthError> {
    let mut retried = false;
    loop {
        let backend = store.resolve_store()?;
        let result = build_keyring_entry(backend.as_ref(), key).and_then(|entry| operation(&entry));
        match result {
            Ok(value) => return Ok(value),
            Err(err) if !retried && is_reconnectable(&err) => {
                store.invalidate_store(&backend);
                retried = true;
            }
            Err(err) => return Err(map_keyring_err(err)),
        }
    }
}

fn is_reconnectable(error: &KeyringError) -> bool {
    matches!(error, KeyringError::PlatformFailure(_))
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
fn build_keyring_entry(store: &KeyringCredentialStore, key: &str) -> Result<Entry, KeyringError> {
    store.build(KEYCHAIN_SERVICE, key, None)
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn build_keyring_entry(store: &KeyringCredentialStore, key: &str) -> Result<Entry, KeyringError> {
    if store.as_any().is::<keyring_core::mock::Store>() {
        return store.build(KEYCHAIN_SERVICE, key, None);
    }

    let label = format!("Aether OAuth: {key}");
    let modifiers = std::collections::HashMap::from([("label", label.as_str())]);
    store.build(KEYCHAIN_SERVICE, key, Some(&modifiers))
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
    use crate::OAuthCredential;
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
        assert!(store.contains("server"));

        store.delete("server").await.unwrap();
        assert!(!store.contains("server"));
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
        let store = OsKeyringStore::from_factory(Arc::new(|| Err(OAuthError::CredentialStore("no dbus".to_string()))));

        let load_err = store.load_credential("k").await.unwrap_err();
        assert!(matches!(load_err, OAuthError::CredentialStore(m) if m.contains("no dbus")));

        let save_err = store.save_credential("k", credential()).await.unwrap_err();
        assert!(matches!(save_err, OAuthError::CredentialStore(m) if m.contains("no dbus")));

        let delete_err = store.delete("k").await.unwrap_err();
        assert!(matches!(delete_err, OAuthError::CredentialStore(m) if m.contains("no dbus")));

        assert!(!store.contains("k"));
    }

    #[tokio::test]
    async fn backend_construction_failure_is_retried() {
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = Arc::clone(&counter);
        let store = OsKeyringStore::from_factory(Arc::new(move || {
            counter_clone.fetch_add(1, Ordering::SeqCst);
            Err(OAuthError::CredentialStore("no dbus".to_string()))
        }));

        let _ = store.load_credential("k").await;
        let _ = store.load_credential("k").await;

        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn platform_failure_reconnects_and_retries_the_operation() {
        use keyring_core::api::{CredentialApi, CredentialStoreApi};
        use std::any::Any;
        use std::collections::HashMap;

        struct FailingStore;
        struct FailingCredential;

        impl CredentialStoreApi for FailingStore {
            fn vendor(&self) -> String {
                "failing".to_string()
            }

            fn id(&self) -> String {
                "failing".to_string()
            }

            fn build(&self, _: &str, _: &str, _: Option<&HashMap<&str, &str>>) -> keyring_core::Result<Entry> {
                Ok(Entry::new_with_credential(Arc::new(FailingCredential)))
            }

            fn as_any(&self) -> &dyn Any {
                self
            }
        }

        impl CredentialApi for FailingCredential {
            fn set_secret(&self, _: &[u8]) -> keyring_core::Result<()> {
                Err(platform_failure())
            }

            fn get_secret(&self) -> keyring_core::Result<Vec<u8>> {
                Err(platform_failure())
            }

            fn delete_credential(&self) -> keyring_core::Result<()> {
                Err(platform_failure())
            }

            fn get_credential(&self) -> keyring_core::Result<Option<Arc<keyring_core::Credential>>> {
                Err(platform_failure())
            }

            fn get_specifiers(&self) -> Option<(String, String)> {
                Some((KEYCHAIN_SERVICE.to_string(), "server".to_string()))
            }

            fn as_any(&self) -> &dyn Any {
                self
            }
        }

        fn platform_failure() -> KeyringError {
            KeyringError::PlatformFailure(Box::new(std::io::Error::other("The session does not exist")))
        }

        let healthy = keyring_core::mock::Store::new().unwrap();
        let entry = healthy.build(KEYCHAIN_SERVICE, "server", None).unwrap();
        entry.set_secret(&serde_json::to_vec(&credential()).unwrap()).unwrap();
        let attempts = Arc::new(AtomicUsize::new(0));
        let factory_attempts = Arc::clone(&attempts);
        let store = OsKeyringStore::from_factory(Arc::new(move || {
            if factory_attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok(Arc::new(FailingStore))
            } else {
                Ok(healthy.clone())
            }
        }));

        let loaded = store.load_credential("server").await.unwrap();

        assert_eq!(loaded.unwrap().access_token, "access");
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }
}
