use crate::credential::{OAuthCredential, OAuthCredentialStorage};
use crate::error::OAuthError;
use age::scrypt::Identity;
use age::secrecy::SecretString;
use age::{Decryptor, Encryptor};
use async_trait::async_trait;
use dirs::home_dir;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env::var;
use std::io::{Read, Write};
use std::iter::once;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const DEFAULT_PASSWORD_ENV: &str = "AETHER_CREDENTIALS_PASSWORD";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CredentialStore {
    credentials: HashMap<String, OAuthCredential>,
}

pub struct EncryptedFileOAuthCredentialStorage {
    path: PathBuf,
    passphrase: String,
    write_guard: Mutex<()>,
}

impl EncryptedFileOAuthCredentialStorage {
    pub fn new(path: PathBuf, passphrase: String) -> Self {
        Self { path, passphrase, write_guard: Mutex::new(()) }
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

    fn encrypt(plaintext: &[u8], passphrase: &str) -> Result<Vec<u8>, OAuthError> {
        let fail = |e| OAuthError::CredentialStore(format!("Encryption failed: {e}"));

        let mut ciphertext = Vec::new();
        let mut writer = Encryptor::with_user_passphrase(SecretString::from(passphrase))
            .wrap_output(&mut ciphertext)
            .map_err(fail)?;
        writer.write_all(plaintext).map_err(fail)?;
        writer.finish().map_err(fail)?;

        Ok(ciphertext)
    }

    fn decrypt(ciphertext: &[u8], passphrase: &str) -> Result<Vec<u8>, OAuthError> {
        let decryptor = Decryptor::new(ciphertext)
            .map_err(|e| OAuthError::CredentialStore(format!("Invalid encrypted file: {e}")))?;

        let mut reader =
            decryptor.decrypt(once(&Identity::new(SecretString::from(passphrase)) as &dyn age::Identity)).map_err(
                |e| OAuthError::CredentialStore(format!("Decryption failed — wrong passphrase or corrupted file: {e}")),
            )?;

        let mut plaintext = Vec::new();
        reader
            .read_to_end(&mut plaintext)
            .map_err(|e| OAuthError::CredentialStore(format!("Decryption failed: {e}")))?;

        Ok(plaintext)
    }

    fn load(&self) -> Result<CredentialStore, OAuthError> {
        if !self.path.exists() {
            return Ok(CredentialStore { credentials: HashMap::new() });
        }

        let bytes = std::fs::read(&self.path)?;
        if bytes.is_empty() {
            return Ok(CredentialStore { credentials: HashMap::new() });
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

        let mut store = self.load()?;
        mutate(&mut store);

        let plaintext = serde_json::to_vec(&store)
            .map_err(|e| OAuthError::CredentialStore(format!("Failed to serialize credentials: {e}")))?;

        let ciphertext = Self::encrypt(&plaintext, &self.passphrase)?;
        write_atomic(&self.path, &ciphertext)
    }
}

fn default_path() -> Result<PathBuf, OAuthError> {
    home_dir().map(|home| home.join(".aether").join("credentials.enc")).ok_or_else(|| {
        OAuthError::CredentialStore(
            "Could not determine the home directory for the encrypted credential file".to_string(),
        )
    })
}

fn write_atomic(path: &Path, data: &[u8]) -> Result<(), OAuthError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let temp_path = path.with_extension("tmp");

    {
        let mut file = std::fs::File::create(&temp_path)?;
        file.write_all(data)?;
        file.sync_all()?;
    }

    std::fs::rename(&temp_path, path)?;

    Ok(())
}

#[async_trait]
impl OAuthCredentialStorage for EncryptedFileOAuthCredentialStorage {
    async fn load_credential(&self, key: &str) -> Result<Option<OAuthCredential>, OAuthError> {
        let store = self.load()?;
        Ok(store.credentials.get(key).cloned())
    }

    async fn save_credential(&self, key: &str, credential: OAuthCredential) -> Result<(), OAuthError> {
        self.update(|store| {
            store.credentials.insert(key.to_string(), credential);
        })
    }

    async fn delete_credential(&self, key: &str) -> Result<(), OAuthError> {
        self.update(|store| {
            store.credentials.remove(key);
        })
    }

    fn has_credential(&self, key: &str) -> bool {
        self.load().is_ok_and(|store| store.credentials.contains_key(key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn save_then_load_round_trips() {
        let store = temp_store("correct-passphrase");
        let cred = test_credential();

        store.save_credential("server-1", cred.clone()).await.unwrap();
        let loaded = store.load_credential("server-1").await.unwrap().unwrap();

        assert_eq!(loaded.client_id, "client_1");
        assert_eq!(loaded.access_token, "tok_abc");
        assert_eq!(loaded.refresh_token.as_deref(), Some("ref_xyz"));
        assert_eq!(loaded.granted_scopes, vec!["scope1"]);
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
        assert!(store.has_credential("server-1"));

        store.delete_credential("server-1").await.unwrap();
        assert!(!store.has_credential("server-1"));
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
            granted_scopes: vec![],
        };
        let cred_b = OAuthCredential {
            client_id: "b".to_string(),
            access_token: "token_b".to_string(),
            refresh_token: None,
            expires_at: None,
            granted_scopes: vec![],
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
            granted_scopes: vec![],
        };
        let cred_v2 = OAuthCredential {
            client_id: "c".to_string(),
            access_token: "v2".to_string(),
            refresh_token: None,
            expires_at: None,
            granted_scopes: vec![],
        };

        store.save_credential("server", cred_v1).await.unwrap();
        store.save_credential("server", cred_v2).await.unwrap();

        let loaded = store.load_credential("server").await.unwrap().unwrap();
        assert_eq!(loaded.access_token, "v2");
    }

    #[test]
    fn encrypt_decrypt_round_trips() {
        let plaintext = b"hello, world!";
        let ciphertext = EncryptedFileOAuthCredentialStorage::encrypt(plaintext, "passphrase").unwrap();
        let decrypted = EncryptedFileOAuthCredentialStorage::decrypt(&ciphertext, "passphrase").unwrap();
        assert_eq!(decrypted, plaintext);
    }

    fn test_credential() -> OAuthCredential {
        OAuthCredential {
            client_id: "client_1".to_string(),
            access_token: "tok_abc".to_string(),
            refresh_token: Some("ref_xyz".to_string()),
            expires_at: Some(9_999_999_999_999),
            granted_scopes: vec!["scope1".to_string()],
        }
    }

    fn temp_store(passphrase: &str) -> EncryptedFileOAuthCredentialStorage {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.keep().join("creds.enc");
        EncryptedFileOAuthCredentialStorage::new(path, passphrase.to_string())
    }
}
