use crate::credential::OAuthCredentialStorage;
use crate::error::OAuthError;
use age::scrypt::{Identity, Recipient};
use age::secrecy::SecretString;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::env::var;
use std::fs::{File, OpenOptions, create_dir_all};
use std::io::ErrorKind;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::SystemTime;
use utils::fs::write_atomic;
use utils::path::aether_home;

const DEFAULT_PASSWORD_ENV: &str = "AETHER_CREDENTIALS_PASSWORD";

pub struct EncryptedFileCredentialStore {
    file: EncryptedFile,
}

impl EncryptedFileCredentialStore {
    pub fn new(path: PathBuf, passphrase: String) -> Self {
        Self { file: EncryptedFile { path, passphrase, scrypt_work_factor: None, cache: Arc::default() } }
    }

    pub fn with_scrypt_work_factor(mut self, log_n: u8) -> Self {
        self.file.scrypt_work_factor = Some(log_n);
        self
    }

    pub fn from_settings(path: Option<PathBuf>, password_env: Option<&str>) -> Result<Self, OAuthError> {
        let path = path.map_or_else(default_path, Ok)?;
        let env_var = password_env.unwrap_or(DEFAULT_PASSWORD_ENV);
        let passphrase = var(env_var).ok().filter(|pass| !pass.is_empty()).ok_or_else(|| {
            OAuthError::CredentialStore(format!(
                "Encrypted file credential store requires a passphrase. \
                     Set the {env_var} environment variable or configure a custom `passwordEnv` in settings."
            ))
        })?;

        Ok(Self::new(path, passphrase))
    }

    /// Runs blocking file work off the async runtime.
    async fn with_file<T: Send + 'static>(
        &self,
        work: impl FnOnce(&EncryptedFile) -> Result<T, OAuthError> + Send + 'static,
    ) -> Result<T, OAuthError> {
        let file = self.file.clone();
        tokio::task::spawn_blocking(move || work(&file))
            .await
            .map_err(|error| OAuthError::CredentialStore(format!("Credential store task failed: {error}")))?
    }
}

#[async_trait]
impl OAuthCredentialStorage for EncryptedFileCredentialStore {
    async fn load(&self, key: &str) -> Result<Option<serde_json::Value>, OAuthError> {
        let key = key.to_string();
        self.with_file(move |file| Ok(file.read()?.credentials.get(&key).cloned())).await
    }

    async fn save(&self, key: &str, value: serde_json::Value) -> Result<(), OAuthError> {
        let key = key.to_string();
        self.with_file(move |file| {
            file.update(|store| {
                store.credentials.insert(key, value);
            })
        })
        .await
    }

    async fn delete(&self, key: &str) -> Result<(), OAuthError> {
        let key = key.to_string();
        self.with_file(move |file| {
            file.update(|store| {
                store.credentials.remove(&key);
            })
        })
        .await
    }

    fn contains(&self, key: &str) -> bool {
        self.file.read().is_ok_and(|store| store.credentials.contains_key(key))
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct CredentialStore {
    credentials: HashMap<String, Value>,
}

#[derive(Clone)]
struct EncryptedFile {
    path: PathBuf,
    passphrase: String,
    scrypt_work_factor: Option<u8>,
    cache: Arc<Mutex<Option<Decrypted>>>,
}

struct Decrypted {
    version: Option<FileVersion>,
    store: CredentialStore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileVersion {
    modified: SystemTime,
    len: u64,
    #[cfg(unix)]
    inode: u64,
}

impl EncryptedFile {
    fn read(&self) -> Result<CredentialStore, OAuthError> {
        let version = file_version(&self.path)?;
        let mut cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(decrypted) = cache.as_ref().filter(|decrypted| decrypted.version == version) {
            return Ok(decrypted.store.clone());
        }
        let store = self.decrypt_file(version)?;
        *cache = Some(Decrypted { version, store: store.clone() });
        Ok(store)
    }

    fn update(&self, mutate: impl FnOnce(&mut CredentialStore)) -> Result<(), OAuthError> {
        let _lock = self.lock()?;
        let mut store = self.read()?;
        mutate(&mut store);

        let plaintext = serde_json::to_vec(&store)
            .map_err(|e| OAuthError::CredentialStore(format!("Failed to serialize credentials: {e}")))?;
        write_atomic(&self.path, &encrypt(&plaintext, &self.passphrase, self.scrypt_work_factor)?)?;

        let version = file_version(&self.path)?;
        *self.cache.lock().unwrap_or_else(PoisonError::into_inner) = Some(Decrypted { version, store });
        Ok(())
    }

    fn decrypt_file(&self, version: Option<FileVersion>) -> Result<CredentialStore, OAuthError> {
        if version.is_none_or(|version| version.len == 0) {
            return Ok(CredentialStore::default());
        }
        let plaintext = decrypt(&std::fs::read(&self.path)?, &self.passphrase)?;
        serde_json::from_slice(&plaintext)
            .map_err(|e| OAuthError::CredentialStore(format!("Invalid credential data: {e}")))
    }

    fn lock(&self) -> Result<File, OAuthError> {
        if let Some(parent) = self.path.parent() {
            create_dir_all(parent)?;
        }
        let mut lock_path = self.path.clone().into_os_string();
        lock_path.push(".lock");
        let lock = OpenOptions::new().create(true).truncate(false).write(true).open(lock_path)?;
        lock.lock()?;
        Ok(lock)
    }
}

fn file_version(path: &Path) -> Result<Option<FileVersion>, OAuthError> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    Ok(Some(FileVersion {
        modified: metadata.modified()?,
        len: metadata.len(),
        #[cfg(unix)]
        inode: metadata.ino(),
    }))
}

fn encrypt(plaintext: &[u8], passphrase: &str, work_factor: Option<u8>) -> Result<Vec<u8>, OAuthError> {
    let mut recipient = Recipient::new(SecretString::from(passphrase));
    if let Some(log_n) = work_factor {
        recipient.set_work_factor(log_n);
    }
    age::encrypt(&recipient, plaintext).map_err(|e| OAuthError::CredentialStore(format!("Encryption failed: {e}")))
}

fn decrypt(ciphertext: &[u8], passphrase: &str) -> Result<Vec<u8>, OAuthError> {
    age::decrypt(&Identity::new(SecretString::from(passphrase)), ciphertext).map_err(|e| {
        OAuthError::CredentialStore(format!("Decryption failed — wrong passphrase or corrupted file: {e}"))
    })
}

fn default_path() -> Result<PathBuf, OAuthError> {
    aether_home().map(|home| home.join("credentials.enc")).ok_or_else(|| {
        OAuthError::CredentialStore(
            "Could not determine the Aether home directory for the encrypted credential file".to_string(),
        )
    })
}

#[cfg(test)]
mod tests {
    use std::fs::write;

    use super::*;
    use crate::OAuthCredential;
    use crate::credential::assert_storage_contract;
    use tempfile::TempDir;

    /// `N = 2^10`, fast enough that the KDF stops dominating the suite.
    const TEST_WORK_FACTOR: u8 = 10;

    #[tokio::test]
    async fn satisfies_the_storage_contract() {
        let (_dir, store) = temp_store("pass");
        assert_storage_contract(&store).await;
    }

    #[tokio::test]
    async fn credentials_persist_across_instances() {
        let (dir, store) = temp_store("pass");
        store.save_credential("server-1", test_credential()).await.unwrap();

        let reopened = store_at(&dir, "pass");

        assert_eq!(reopened.load_credential("server-1").await.unwrap(), Some(test_credential()));
    }

    #[tokio::test]
    async fn a_save_through_one_instance_is_visible_to_another_that_already_read() {
        let (dir, first) = temp_store("pass");
        let second = store_at(&dir, "pass");
        assert_eq!(second.load_credential("server-1").await.unwrap(), None);

        first.save_credential("server-1", test_credential()).await.unwrap();

        assert_eq!(second.load_credential("server-1").await.unwrap(), Some(test_credential()));
        assert!(second.contains("server-1"));
    }

    #[tokio::test]
    async fn concurrent_saves_from_two_instances_keep_both_credentials() {
        let (dir, first) = temp_store("pass");
        let second = store_at(&dir, "pass");

        let (saved_first, saved_second) = tokio::join!(
            first.save_credential("server-1", test_credential()),
            second.save_credential("server-2", test_credential()),
        );
        saved_first.unwrap();
        saved_second.unwrap();

        let reopened = store_at(&dir, "pass");
        assert!(reopened.contains("server-1") && reopened.contains("server-2"));
    }

    #[tokio::test]
    async fn wrong_passphrase_fails_to_load() {
        let (dir, store) = temp_store("correct-pass");
        store.save_credential("server-1", test_credential()).await.unwrap();

        let wrong_store = store_at(&dir, "wrong-pass");

        let err = wrong_store.load_credential("server-1").await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Decryption failed"), "Expected decryption error, got: {msg}");
    }

    #[tokio::test]
    async fn loads_existing_encrypted_file_format() {
        let (dir, store) = temp_store("pass");
        let plaintext = serde_json::to_vec(&serde_json::json!({
            "credentials": {
                "codex": {
                    "client_id": "client_1",
                    "access_token": "tok_abc",
                    "refresh_token": "ref_xyz",
                    "expires_at": 9_999_999_999_999_u64
                }
            }
        }))
        .unwrap();
        let ciphertext = encrypt(&plaintext, "pass", Some(TEST_WORK_FACTOR)).unwrap();

        write(dir.path().join("creds.enc"), ciphertext).unwrap();
        let credential = store.load_credential("codex").await.unwrap().expect("existing credential");
        assert_eq!(credential, test_credential());
    }

    fn test_credential() -> OAuthCredential {
        OAuthCredential::new("client_1", "tok_abc").refresh_token("ref_xyz").expires_at(9_999_999_999_999)
    }

    fn temp_store(passphrase: &str) -> (TempDir, EncryptedFileCredentialStore) {
        let dir = TempDir::new().unwrap();
        let store = store_at(&dir, passphrase);
        (dir, store)
    }

    fn store_at(dir: &TempDir, passphrase: &str) -> EncryptedFileCredentialStore {
        EncryptedFileCredentialStore::new(dir.path().join("creds.enc"), passphrase.to_string())
            .with_scrypt_work_factor(TEST_WORK_FACTOR)
    }
}
