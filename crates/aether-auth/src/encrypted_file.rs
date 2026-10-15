use crate::credential::OAuthCredentialStorage;
use crate::error::OAuthError;
use age::scrypt::{Identity, Recipient};
use age::secrecy::SecretString;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::env::var;
use std::path::PathBuf;
use std::sync::Mutex;
use utils::fs::write_atomic;
use utils::settings::aether_home;

const DEFAULT_PASSWORD_ENV: &str = "AETHER_CREDENTIALS_PASSWORD";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct CredentialStore {
    credentials: HashMap<String, Value>,
}

pub struct EncryptedFileOAuthCredentialStorage {
    path: PathBuf,
    passphrase: String,
    scrypt_work_factor: Option<u8>,
    write_guard: Mutex<()>,
}

impl EncryptedFileOAuthCredentialStorage {
    pub fn new(path: PathBuf, passphrase: String) -> Self {
        Self { path, passphrase, scrypt_work_factor: None, write_guard: Mutex::new(()) }
    }

    pub fn with_scrypt_work_factor(mut self, log_n: u8) -> Self {
        self.scrypt_work_factor = Some(log_n);
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

    fn load_store(&self) -> Result<CredentialStore, OAuthError> {
        if !self.path.exists() {
            return Ok(CredentialStore::default());
        }

        let bytes = std::fs::read(&self.path)?;
        if bytes.is_empty() {
            return Ok(CredentialStore::default());
        }

        let plaintext = Self::decrypt(&bytes, &self.passphrase)?;
        serde_json::from_slice(&plaintext)
            .map_err(|e| OAuthError::CredentialStore(format!("Invalid credential data: {e}")))
    }

    fn update(&self, mutate: impl FnOnce(&mut CredentialStore)) -> Result<(), OAuthError> {
        let _guard = self
            .write_guard
            .lock()
            .map_err(|_| OAuthError::CredentialStore("Failed to acquire write lock on credential store".to_string()))?;

        let mut store = self.load_store()?;
        mutate(&mut store);

        let plaintext = serde_json::to_vec(&store)
            .map_err(|e| OAuthError::CredentialStore(format!("Failed to serialize credentials: {e}")))?;

        let ciphertext = Self::encrypt(&plaintext, &self.passphrase, self.scrypt_work_factor)?;
        Ok(write_atomic(&self.path, &ciphertext)?)
    }
}

fn default_path() -> Result<PathBuf, OAuthError> {
    aether_home().map(|home| home.join("credentials.enc")).ok_or_else(|| {
        OAuthError::CredentialStore(
            "Could not determine the Aether home directory for the encrypted credential file".to_string(),
        )
    })
}

#[async_trait]
impl OAuthCredentialStorage for EncryptedFileOAuthCredentialStorage {
    async fn load(&self, key: &str) -> Result<Option<serde_json::Value>, OAuthError> {
        Ok(self.load_store()?.credentials.get(key).cloned())
    }

    async fn save(&self, key: &str, value: serde_json::Value) -> Result<(), OAuthError> {
        self.update(|store| {
            store.credentials.insert(key.to_string(), value);
        })
    }

    async fn delete(&self, key: &str) -> Result<(), OAuthError> {
        self.update(|store| {
            store.credentials.remove(key);
        })
    }

    fn contains(&self, key: &str) -> bool {
        self.load_store().is_ok_and(|store| store.credentials.contains_key(key))
    }
}

#[cfg(test)]
mod tests {
    use std::fs::{create_dir, write};

    use super::*;
    use crate::OAuthCredential;
    use crate::credential::assert_storage_contract;

    /// `N = 2^10`, fast enough that the KDF stops dominating the suite.
    const TEST_WORK_FACTOR: u8 = 10;

    #[tokio::test]
    async fn satisfies_the_storage_contract() {
        assert_storage_contract(&temp_store("pass")).await;
    }

    #[tokio::test]
    async fn credentials_persist_across_instances() {
        let store = temp_store("pass");
        store.save_credential("server-1", test_credential()).await.unwrap();

        let reopened = EncryptedFileOAuthCredentialStorage::new(store.path.clone(), "pass".to_string());

        assert_eq!(reopened.load_credential("server-1").await.unwrap(), Some(test_credential()));
    }

    #[tokio::test]
    async fn wrong_passphrase_fails_to_load() {
        let store = temp_store("correct-pass");
        store.save_credential("server-1", test_credential()).await.unwrap();

        let wrong_store = EncryptedFileOAuthCredentialStorage::new(store.path.clone(), "wrong-pass".to_string());

        let err = wrong_store.load_credential("server-1").await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Decryption failed"), "Expected decryption error, got: {msg}");
    }

    #[tokio::test]
    async fn loads_existing_encrypted_file_format() {
        let store = temp_store("pass");
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
        let ciphertext =
            EncryptedFileOAuthCredentialStorage::encrypt(&plaintext, "pass", Some(TEST_WORK_FACTOR)).unwrap();

        write(&store.path, ciphertext).unwrap();
        let credential = store.load_credential("codex").await.unwrap().expect("existing credential");
        assert_eq!(credential, test_credential());
    }

    #[tokio::test]
    async fn save_succeeds_while_a_temp_path_is_occupied() {
        let store = temp_store("pass");
        create_dir(store.path.with_extension("tmp")).unwrap();
        store.save_credential("server-1", test_credential()).await.unwrap();
        assert!(store.load_credential("server-1").await.unwrap().is_some());
    }

    fn test_credential() -> OAuthCredential {
        OAuthCredential::new("client_1", "tok_abc").refresh_token("ref_xyz").expires_at(9_999_999_999_999)
    }

    fn temp_store(passphrase: &str) -> EncryptedFileOAuthCredentialStorage {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.keep().join("creds.enc");
        EncryptedFileOAuthCredentialStorage::new(path, passphrase.to_string()).with_scrypt_work_factor(TEST_WORK_FACTOR)
    }
}
