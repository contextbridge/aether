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
    use crate::credential::assert_storage_contract;
    use serde_json::json;

    #[tokio::test]
    async fn satisfies_the_storage_contract() {
        assert_storage_contract(&FakeOAuthCredentialStore::new()).await;
    }

    #[tokio::test]
    async fn builders_seed_the_store() {
        let credential = OAuthCredential::new("client", "token");
        let store = FakeOAuthCredentialStore::new()
            .with_credential("codex", credential.clone())
            .with_value("mcp:slack", json!(1));

        assert_eq!(store.load_credential("codex").await.unwrap(), Some(credential));
        assert_eq!(store.load("mcp:slack").await.unwrap(), Some(json!(1)));
    }
}
