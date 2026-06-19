use async_trait::async_trait;
use oauth2::{AccessToken, RefreshToken};
use rmcp::transport::auth::{
    AuthError, CredentialStore, OAuthTokenResponse, StoredCredentials, VendorExtraTokenFields,
};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::{OAuthCredential, OAuthCredentialStorage};

/// Per-server adapter that binds an [`OAuthCredentialStorage`] to a single MCP server id
/// and implements `rmcp::transport::auth::CredentialStore`.
#[derive(Clone)]
pub struct McpCredentialStore {
    server_id: String,
    store: Arc<dyn OAuthCredentialStorage>,
    now_fn: fn() -> SystemTime,
}

impl McpCredentialStore {
    pub fn new(store: Arc<dyn OAuthCredentialStorage>, server_id: String) -> Self {
        Self { server_id, store, now_fn: SystemTime::now }
    }

    pub fn with_now_fn(mut self, f: fn() -> SystemTime) -> Self {
        self.now_fn = f;
        self
    }

    fn now(&self) -> SystemTime {
        (self.now_fn)()
    }
}

#[async_trait]
impl CredentialStore for McpCredentialStore {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        let cred =
            self.store.load_credential(&self.server_id).await.map_err(|e| AuthError::InternalError(e.to_string()))?;

        let now = self.now();
        Ok(cred.map(|c| {
            let token_received_at = c.expires_at.map(|_| now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs());
            let token_response = build_token_response(&c, now);
            StoredCredentials::new(c.client_id, Some(token_response), c.granted_scopes, token_received_at)
        }))
    }

    async fn save(&self, credentials: StoredCredentials) -> Result<(), AuthError> {
        let token = credentials
            .token_response
            .ok_or_else(|| AuthError::InternalError("No token response to save".to_string()))?;

        let preserved_refresh_token = self
            .store
            .load_credential(&self.server_id)
            .await
            .map_err(|e| AuthError::InternalError(e.to_string()))?
            .and_then(|cred| (cred.client_id == credentials.client_id).then_some(cred.refresh_token).flatten());
        let credential = OAuthCredential::from_token_response(credentials.client_id, &token);
        let credential = OAuthCredential {
            refresh_token: credential.refresh_token.or(preserved_refresh_token),
            granted_scopes: credentials.granted_scopes,
            ..credential
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

fn build_token_response(cred: &OAuthCredential, now: SystemTime) -> OAuthTokenResponse {
    let mut response = OAuthTokenResponse::new(
        AccessToken::new(cred.access_token.clone()),
        oauth2::basic::BasicTokenType::Bearer,
        VendorExtraTokenFields::default(),
    );

    if let Some(ref refresh) = cred.refresh_token {
        response.set_refresh_token(Some(RefreshToken::new(refresh.clone())));
    }

    if let Some(expires_at_millis) = cred.expires_at {
        let expires_at = UNIX_EPOCH + Duration::from_millis(expires_at_millis);
        let remaining = expires_at.duration_since(now).unwrap_or_default();
        response.set_expires_in(Some(&remaining));
    }

    response
}

#[cfg(test)]
mod tests {
    use oauth2::TokenResponse;
    use oauth2::basic::BasicTokenType;

    use super::*;
    use crate::FakeOAuthCredentialStore;

    const FAKE_NOW_SECS: u64 = 2_000_000;

    fn fake_now() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(FAKE_NOW_SECS)
    }

    fn mcp_store(store: Arc<FakeOAuthCredentialStore>) -> McpCredentialStore {
        McpCredentialStore::new(store, "server".to_string()).with_now_fn(fake_now)
    }

    fn credential_expiring_at(expires_at: SystemTime) -> OAuthCredential {
        OAuthCredential {
            client_id: "client".to_string(),
            access_token: "access".to_string(),
            refresh_token: Some("refresh".to_string()),
            expires_at: Some(systime_to_millis(expires_at)),
            granted_scopes: Vec::new(),
        }
    }

    fn systime_to_millis(t: SystemTime) -> u64 {
        u64::try_from(t.duration_since(UNIX_EPOCH).unwrap_or_default().as_millis()).unwrap_or(u64::MAX)
    }

    #[tokio::test]
    async fn mcp_store_round_trips_stored_credentials() {
        let store = Arc::new(FakeOAuthCredentialStore::new());
        let mcp_store = mcp_store(store.clone());
        let cred = credential_expiring_at(fake_now());
        let token_response = build_token_response(&cred, fake_now());
        let stored = StoredCredentials::new(cred.client_id, Some(token_response), Vec::new(), Some(FAKE_NOW_SECS));

        CredentialStore::save(&mcp_store, stored).await.unwrap();

        let loaded = CredentialStore::load(&mcp_store).await.unwrap().unwrap();
        let token = loaded.token_response.unwrap();
        assert_eq!(loaded.client_id, "client");
        assert_eq!(token.access_token().secret(), "access");
        assert_eq!(token.refresh_token().map(|t| t.secret().as_str()), Some("refresh"));
    }

    #[tokio::test]
    async fn mcp_store_preserves_existing_refresh_token_when_save_omits_one() {
        let store = Arc::new(FakeOAuthCredentialStore::new());
        store.save_credential("server", credential_expiring_at(fake_now())).await.unwrap();
        let mcp_store = mcp_store(store.clone());
        let stored = StoredCredentials::new(
            "client".to_string(),
            Some(OAuthTokenResponse::new(
                AccessToken::new("token".to_string()),
                BasicTokenType::Bearer,
                VendorExtraTokenFields::default(),
            )),
            Vec::new(),
            Some(FAKE_NOW_SECS),
        );

        CredentialStore::save(&mcp_store, stored).await.unwrap();
        let loaded = CredentialStore::load(&mcp_store).await.unwrap().unwrap();
        let token = loaded.token_response.unwrap();
        assert_eq!(token.access_token().secret(), "token");
        assert_eq!(token.refresh_token().map(|t| t.secret().as_str()), Some("refresh"));
    }

    #[tokio::test]
    async fn mcp_store_clear_removes_credential() {
        let store = Arc::new(FakeOAuthCredentialStore::new());
        store.save_credential("server", credential_expiring_at(fake_now())).await.unwrap();

        let mcp_store = mcp_store(store.clone());
        CredentialStore::clear(&mcp_store).await.unwrap();

        assert!(store.load_credential("server").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn load_populates_token_received_at_when_expiry_info_present() {
        let store = Arc::new(FakeOAuthCredentialStore::new());
        store.save_credential("server", credential_expiring_at(fake_now() + Duration::from_hours(1))).await.unwrap();

        let loaded = CredentialStore::load(&mcp_store(store)).await.unwrap().unwrap();

        assert_eq!(loaded.token_received_at, Some(FAKE_NOW_SECS));
    }

    #[tokio::test]
    async fn load_omits_token_received_at_when_no_expiry_info() {
        let cred = OAuthCredential {
            client_id: "client".to_string(),
            access_token: "access".to_string(),
            refresh_token: Some("refresh".to_string()),
            expires_at: None,
            granted_scopes: Vec::new(),
        };
        let store = Arc::new(FakeOAuthCredentialStore::new());
        store.save_credential("server", cred).await.unwrap();

        let loaded = CredentialStore::load(&mcp_store(store)).await.unwrap().unwrap();

        assert!(loaded.token_received_at.is_none());
        assert!(loaded.token_response.unwrap().expires_in().is_none());
    }

    #[tokio::test]
    async fn expired_credential_with_refresh_token_sets_zero_expires_in() {
        let store = Arc::new(FakeOAuthCredentialStore::new());
        store.save_credential("server", credential_expiring_at(fake_now() - Duration::from_mins(10))).await.unwrap();

        let loaded = CredentialStore::load(&mcp_store(store)).await.unwrap().unwrap();
        let token = loaded.token_response.as_ref().unwrap();

        assert_eq!(token.expires_in(), Some(Duration::ZERO));
        assert_eq!(loaded.token_received_at, Some(FAKE_NOW_SECS));
        assert_eq!(token.refresh_token().map(|t| t.secret().as_str()), Some("refresh"));
    }

    #[tokio::test]
    async fn unexpired_credential_sets_exact_remaining_expires_in() {
        let store = Arc::new(FakeOAuthCredentialStore::new());
        store.save_credential("server", credential_expiring_at(fake_now() + Duration::from_hours(1))).await.unwrap();

        let loaded = CredentialStore::load(&mcp_store(store)).await.unwrap().unwrap();
        let token = loaded.token_response.as_ref().unwrap();

        assert_eq!(token.expires_in(), Some(Duration::from_hours(1)));
    }

    #[tokio::test]
    async fn load_round_trips_granted_scopes() {
        let cred = OAuthCredential {
            granted_scopes: vec!["read".to_string(), "write".to_string()],
            ..credential_expiring_at(fake_now() + Duration::from_hours(1))
        };
        let store = Arc::new(FakeOAuthCredentialStore::new());
        store.save_credential("server", cred).await.unwrap();

        let loaded = CredentialStore::load(&mcp_store(store)).await.unwrap().unwrap();

        assert_eq!(loaded.granted_scopes, vec!["read".to_string(), "write".to_string()]);
    }

    #[tokio::test]
    async fn save_persists_granted_scopes_from_rmcp() {
        let store = Arc::new(FakeOAuthCredentialStore::new());
        let mcp_store = mcp_store(store.clone());
        let token_response = OAuthTokenResponse::new(
            AccessToken::new("access".to_string()),
            BasicTokenType::Bearer,
            VendorExtraTokenFields::default(),
        );
        let stored = StoredCredentials::new(
            "client".to_string(),
            Some(token_response),
            vec!["read".to_string(), "admin".to_string()],
            Some(FAKE_NOW_SECS),
        );

        CredentialStore::save(&mcp_store, stored).await.unwrap();

        let persisted = store.load_credential("server").await.unwrap().unwrap();
        assert_eq!(persisted.granted_scopes, vec!["read".to_string(), "admin".to_string()]);
    }
}
