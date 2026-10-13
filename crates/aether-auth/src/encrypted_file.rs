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

    /// `N = 2^10`, fast enough that the KDF stops dominating the suite.
    const TEST_WORK_FACTOR: u8 = 10;

    #[tokio::test]
    async fn save_then_load_round_trips() {
        let store = temp_store("correct-passphrase");
        let cred = test_credential();

        store.save_credential("server-1", cred.clone()).await.unwrap();
        let loaded = store.load_credential("server-1").await.unwrap().unwrap();

        assert_eq!(loaded.client_id, "client_1");
        assert_eq!(loaded.access_token, "tok_abc");
        assert_eq!(loaded.refresh_token.as_deref(), Some("ref_xyz"));
    }

    #[tokio::test]
    async fn load_returns_none_for_missing_key() {
        let store = temp_store("pass");
        assert!(store.load_credential("nonexistent").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn delete_removes_credential() {
        let store = temp_store("pass");
        store.save_credential("server-1", test_credential()).await.unwrap();
        assert!(store.contains("server-1"));

        store.delete("server-1").await.unwrap();
        assert!(!store.contains("server-1"));
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
    async fn multiple_credentials_are_isolated() {
        let store = temp_store("pass");

        let cred_a = OAuthCredential {
            client_id: "a".to_string(),
            access_token: "token_a".to_string(),
            refresh_token: None,
            expires_at: None,
        };
        let cred_b = OAuthCredential {
            client_id: "b".to_string(),
            access_token: "token_b".to_string(),
            refresh_token: None,
            expires_at: None,
        };

        store.save_credential("server-a", cred_a).await.unwrap();
        store.save_credential("server-b", cred_b).await.unwrap();

        let loaded_a = store.load_credential("server-a").await.unwrap().unwrap();
        let loaded_b = store.load_credential("server-b").await.unwrap().unwrap();

        assert_eq!(loaded_a.access_token, "token_a");
        assert_eq!(loaded_b.access_token, "token_b");
    }

    #[tokio::test]
    async fn save_overwrites_existing_credential() {
        let store = temp_store("pass");
        let cred_v1 = OAuthCredential {
            client_id: "c".to_string(),
            access_token: "v1".to_string(),
            refresh_token: None,
            expires_at: None,
        };
        let cred_v2 = OAuthCredential {
            client_id: "c".to_string(),
            access_token: "v2".to_string(),
            refresh_token: None,
            expires_at: None,
        };

        store.save_credential("server", cred_v1).await.unwrap();
        store.save_credential("server", cred_v2).await.unwrap();

        let loaded = store.load_credential("server").await.unwrap().unwrap();
        assert_eq!(loaded.access_token, "v2");
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
        assert_eq!(credential.access_token, "tok_abc");
    }

    #[tokio::test]
    async fn secrets_round_trip_through_encrypted_file() {
        let store = temp_store("pass");
        let secret = serde_json::json!({"client_id": "c", "granted_scopes": ["read"]});

        store.save("mcp:slack", secret.clone()).await.unwrap();
        store.save_credential("slack", test_credential()).await.unwrap();
        let reopened = EncryptedFileOAuthCredentialStorage::new(store.path.clone(), "pass".to_string())
            .with_scrypt_work_factor(TEST_WORK_FACTOR);
        assert_eq!(reopened.load("mcp:slack").await.unwrap(), Some(secret));
        assert!(reopened.load_credential("slack").await.unwrap().is_some());

        reopened.delete("mcp:slack").await.unwrap();
        assert!(store.load("mcp:slack").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn save_succeeds_while_a_temp_path_is_occupied() {
        let store = temp_store("pass");
        create_dir(store.path.with_extension("tmp")).unwrap();
        store.save_credential("server-1", test_credential()).await.unwrap();
        assert!(store.load_credential("server-1").await.unwrap().is_some());
    }

    #[test]
    fn encrypt_decrypt_round_trips() {
        let plaintext = b"hello, world!";
        let ciphertext =
            EncryptedFileOAuthCredentialStorage::encrypt(plaintext, "passphrase", Some(TEST_WORK_FACTOR)).unwrap();
        let decrypted = EncryptedFileOAuthCredentialStorage::decrypt(&ciphertext, "passphrase").unwrap();
        assert_eq!(decrypted, plaintext);
    }

    fn test_credential() -> OAuthCredential {
        OAuthCredential {
            client_id: "client_1".to_string(),
            access_token: "tok_abc".to_string(),
            refresh_token: Some("ref_xyz".to_string()),
            expires_at: Some(9_999_999_999_999),
        }
    }

    fn temp_store(passphrase: &str) -> EncryptedFileOAuthCredentialStorage {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.keep().join("creds.enc");
        EncryptedFileOAuthCredentialStorage::new(path, passphrase.to_string()).with_scrypt_work_factor(TEST_WORK_FACTOR)
    }
}
