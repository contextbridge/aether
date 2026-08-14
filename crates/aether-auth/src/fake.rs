use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;

use crate::credential::{OAuthCredential, OAuthCredentialStorage};
use crate::error::OAuthError;

#[derive(Default)]
pub struct FakeOAuthCredentialStore {
    values: Mutex<HashMap<String, serde_json::Value>>,
}

impl FakeOAuthCredentialStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_credential(self, key: &str, credential: OAuthCredential) -> Self {
        self.values.lock().unwrap().insert(key.to_string(), serde_json::to_value(credential).unwrap());
        self
    }

    pub fn with_value(self, key: &str, value: serde_json::Value) -> Self {
        self.values.lock().unwrap().insert(key.to_string(), value);
        self
    }
}

#[async_trait]
impl OAuthCredentialStorage for FakeOAuthCredentialStore {
    async fn load(&self, key: &str) -> Result<Option<serde_json::Value>, OAuthError> {
        Ok(self.values.lock().unwrap().get(key).cloned())
    }

    async fn save(&self, key: &str, value: serde_json::Value) -> Result<(), OAuthError> {
        self.values.lock().unwrap().insert(key.to_string(), value);
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<(), OAuthError> {
        self.values.lock().unwrap().remove(key);
        Ok(())
    }

    fn contains(&self, key: &str) -> bool {
        self.values.lock().unwrap().contains_key(key)
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
        assert!(store.contains("x"));

        store.delete("x").await.unwrap();
        assert!(!store.contains("x"));
    }

    #[tokio::test]
    async fn secrets_round_trip_and_delete() {
        let store = FakeOAuthCredentialStore::new().with_value("mcp:slack", serde_json::json!({"a": 1}));

        assert_eq!(store.load("mcp:slack").await.unwrap(), Some(serde_json::json!({"a": 1})));

        store.save("mcp:slack", serde_json::json!({"a": 2})).await.unwrap();
        assert_eq!(store.load("mcp:slack").await.unwrap(), Some(serde_json::json!({"a": 2})));

        store.delete("mcp:slack").await.unwrap();
        assert!(store.load("mcp:slack").await.unwrap().is_none());
    }

    #[test]
    fn has_value_reflects_state() {
        let store = FakeOAuthCredentialStore::new().with_credential(
            "present",
            OAuthCredential {
                client_id: "c".to_string(),
                access_token: "t".to_string(),
                refresh_token: None,
                expires_at: None,
            },
        );

        assert!(store.contains("present"));
        assert!(!store.contains("absent"));
    }
}
