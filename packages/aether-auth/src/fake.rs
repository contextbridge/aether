use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;

use crate::credential::{OAuthCredential, OAuthCredentialStorage};
use crate::error::OAuthError;

#[derive(Default)]
pub struct FakeOAuthCredentialStore {
    credentials: Mutex<HashMap<String, OAuthCredential>>,
}

impl FakeOAuthCredentialStore {
    pub fn new() -> Self {
        Self { credentials: Mutex::new(HashMap::new()) }
    }

    pub fn with_credential(self, key: &str, credential: OAuthCredential) -> Self {
        self.credentials.lock().unwrap().insert(key.to_string(), credential);
        self
    }
}

#[async_trait]
impl OAuthCredentialStorage for FakeOAuthCredentialStore {
    async fn load_credential(&self, server_id: &str) -> Result<Option<OAuthCredential>, OAuthError> {
        Ok(self.credentials.lock().unwrap().get(server_id).cloned())
    }

    async fn save_credential(&self, key: &str, credential: OAuthCredential) -> Result<(), OAuthError> {
        self.credentials.lock().unwrap().insert(key.to_string(), credential);
        Ok(())
    }

    async fn delete_credential(&self, key: &str) -> Result<(), OAuthError> {
        self.credentials.lock().unwrap().remove(key);
        Ok(())
    }

    fn has_credential(&self, key: &str) -> bool {
        self.credentials.lock().unwrap().contains_key(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn load_returns_none_when_empty() {
        let store = FakeOAuthCredentialStore::new();
        let result = store.load_credential("unknown").await;
        assert!(result.unwrap().is_none());
    }

    #[tokio::test]
    async fn save_then_load_round_trips() {
        let store = FakeOAuthCredentialStore::new();
        let cred = OAuthCredential {
            client_id: "client_1".to_string(),
            access_token: "tok_abc".to_string(),
            refresh_token: Some("ref_xyz".to_string()),
            expires_at: Some(9_999_999_999_999),
        };

        store.save_credential("my-server", cred.clone()).await.unwrap();

        let loaded = store.load_credential("my-server").await.unwrap().expect("should find saved credential");
        assert_eq!(loaded.client_id, "client_1");
        assert_eq!(loaded.access_token, "tok_abc");
        assert_eq!(loaded.refresh_token.as_deref(), Some("ref_xyz"));
    }

    #[tokio::test]
    async fn delete_removes_credential() {
        let store = FakeOAuthCredentialStore::new();
        let cred = OAuthCredential {
            client_id: "c".to_string(),
            access_token: "t".to_string(),
            refresh_token: None,
            expires_at: None,
        };
        store.save_credential("x", cred).await.unwrap();
        assert!(store.has_credential("x"));

        store.delete_credential("x").await.unwrap();
        assert!(!store.has_credential("x"));
    }

    #[test]
    fn has_credential_reflects_state() {
        let store = FakeOAuthCredentialStore::new().with_credential(
            "present",
            OAuthCredential {
                client_id: "c".to_string(),
                access_token: "t".to_string(),
                refresh_token: None,
                expires_at: None,
            },
        );

        assert!(store.has_credential("present"));
        assert!(!store.has_credential("absent"));
    }
}
