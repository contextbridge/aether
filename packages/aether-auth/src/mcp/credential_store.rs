use async_trait::async_trait;
use oauth2::{AccessToken, RefreshToken};
use rmcp::transport::auth::{
    AuthError, CredentialStore, OAuthTokenResponse, StoredCredentials, VendorExtraTokenFields,
};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::{OAuthCredential, OAuthCredentialStorage};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Seconds(pub u64);

/// Per-server adapter that binds an [`OAuthCredentialStorage`] to a single MCP server id
/// and implements `rmcp::transport::auth::CredentialStore`.
#[derive(Clone)]
pub struct McpCredentialStore {
    server_id: String,
    store: Arc<dyn OAuthCredentialStorage>,
    now_fn: fn() -> Seconds,
}

/// Build an `McpCredentialStore` bound to a specific MCP server id.
pub fn mcp_credential_store(store: Arc<dyn OAuthCredentialStorage>, server_id: String) -> McpCredentialStore {
    McpCredentialStore { server_id, store, now_fn: default_now }
}

impl McpCredentialStore {
    pub fn with_now_fn(mut self, f: fn() -> Seconds) -> Self {
        self.now_fn = f;
        self
    }
}

fn default_now() -> Seconds {
    Seconds(SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs())
}

trait StoredCredentialsExt {
    fn from_parts(
        client_id: &str,
        token_response: Option<&OAuthTokenResponse>,
        token_received_at: Option<u64>,
    ) -> StoredCredentials;
}

impl StoredCredentialsExt for StoredCredentials {
    fn from_parts(
        client_id: &str,
        token_response: Option<&OAuthTokenResponse>,
        token_received_at: Option<u64>,
    ) -> StoredCredentials {
        StoredCredentials::new(client_id.to_string(), token_response.cloned(), Vec::new(), token_received_at)
    }
}

#[async_trait]
impl CredentialStore for McpCredentialStore {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        let cred =
            self.store.load_credential(&self.server_id).await.map_err(|e| AuthError::InternalError(e.to_string()))?;

        Ok(cred.map(|c| {
            let token_response = build_token_response(&c);
            let token_received_at = c.expires_at.map(|_| (self.now_fn)().0);
            StoredCredentials::from_parts(&c.client_id, Some(&token_response), token_received_at)
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
        let credential =
            OAuthCredential { refresh_token: credential.refresh_token.or(preserved_refresh_token), ..credential };
        self.store
            .save_credential(&self.server_id, credential)
            .await
            .map_err(|e| AuthError::InternalError(e.to_string()))
    }

    async fn clear(&self) -> Result<(), AuthError> {
        self.store.delete_credential(&self.server_id).await.map_err(|e| AuthError::InternalError(e.to_string()))
    }
}

fn build_token_response(cred: &OAuthCredential) -> OAuthTokenResponse {
    let mut response = OAuthTokenResponse::new(
        AccessToken::new(cred.access_token.clone()),
        oauth2::basic::BasicTokenType::Bearer,
        VendorExtraTokenFields::default(),
    );

    if let Some(ref refresh) = cred.refresh_token {
        response.set_refresh_token(Some(RefreshToken::new(refresh.clone())));
    }

    if let Some(duration) = cred.expires_in() {
        response.set_expires_in(Some(&duration));
    } else if cred.expires_at.is_some() {
        response.set_expires_in(Some(&Duration::ZERO));
    }

    response
}

#[cfg(test)]
mod tests {
    use oauth2::TokenResponse;
    use oauth2::basic::BasicTokenType;

    use super::*;
    use crate::FakeOAuthCredentialStore;

    fn credential() -> OAuthCredential {
        OAuthCredential {
            client_id: "client".to_string(),
            access_token: "access".to_string(),
            refresh_token: Some("refresh".to_string()),
            expires_at: Some(1234),
        }
    }

    #[tokio::test]
    async fn mcp_store_round_trips_stored_credentials() {
        let store: Arc<dyn OAuthCredentialStorage> = Arc::new(FakeOAuthCredentialStore::new());
        let mcp_store = mcp_credential_store(store.clone(), "server".to_string());
        let cred = credential();
        let token_response = build_token_response(&cred);
        let stored = StoredCredentials::from_parts(&cred.client_id, Some(&token_response), Some(default_now().0));

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
        store.save_credential("server", credential()).await.unwrap();
        let mcp_store = mcp_credential_store(store.clone(), "server".to_string());
        let stored = StoredCredentials::from_parts(
            "client",
            Some(&OAuthTokenResponse::new(
                AccessToken::new("token".to_string()),
                BasicTokenType::Bearer,
                VendorExtraTokenFields::default(),
            )),
            Some(default_now().0),
        );

        CredentialStore::save(&mcp_store, stored).await.unwrap();
        let loaded = CredentialStore::load(&mcp_store).await.unwrap().unwrap();
        let token = loaded.token_response.unwrap();
        assert_eq!(token.access_token().secret(), "token");
        assert_eq!(token.refresh_token().map(|t| t.secret().as_str()), Some("refresh"));
    }

    #[tokio::test]
    async fn mcp_store_clear_removes_credential() {
        let store: Arc<dyn OAuthCredentialStorage> = Arc::new(FakeOAuthCredentialStore::new());
        store.save_credential("server", credential()).await.unwrap();

        let mcp_store = mcp_credential_store(store.clone(), "server".to_string());
        CredentialStore::clear(&mcp_store).await.unwrap();

        assert!(store.load_credential("server").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn load_populates_token_received_at_when_expiry_info_present() {
        let store: Arc<dyn OAuthCredentialStorage> = Arc::new(FakeOAuthCredentialStore::new());
        store.save_credential("server", unexpired_credential()).await.unwrap();

        let mcp_store = mcp_credential_store(store.clone(), "server".to_string());
        let loaded = CredentialStore::load(&mcp_store).await.unwrap().unwrap();

        assert!(
            loaded.token_received_at.is_some(),
            "token_received_at must be populated when credential has expiry info"
        );
        let now = default_now().0;
        let received_at = loaded.token_received_at.unwrap();
        assert!(now.abs_diff(received_at) < 5, "token_received_at should be close to current time");
    }

    #[tokio::test]
    async fn load_omits_token_received_at_when_no_expiry_info() {
        let cred = OAuthCredential {
            client_id: "client".to_string(),
            access_token: "access".to_string(),
            refresh_token: Some("refresh".to_string()),
            expires_at: None,
        };
        let store: Arc<dyn OAuthCredentialStorage> = Arc::new(FakeOAuthCredentialStore::new());
        store.save_credential("server", cred).await.unwrap();

        let mcp_store = mcp_credential_store(store.clone(), "server".to_string());
        let loaded = CredentialStore::load(&mcp_store).await.unwrap().unwrap();

        assert!(
            loaded.token_received_at.is_none(),
            "token_received_at should be None when credential has no expiry info"
        );
    }

    #[tokio::test]
    async fn expired_credential_with_refresh_token_sets_zero_expires_in() {
        let store: Arc<dyn OAuthCredentialStorage> = Arc::new(FakeOAuthCredentialStore::new());
        store.save_credential("server", expired_credential_with_refresh()).await.unwrap();

        let mcp_store = mcp_credential_store(store.clone(), "server".to_string());
        let loaded = CredentialStore::load(&mcp_store).await.unwrap().unwrap();
        let token = loaded.token_response.as_ref().unwrap();

        let expires_in = token.expires_in().expect("expires_in must be set for expired tokens with expiry info");
        assert_eq!(expires_in, Duration::ZERO, "expired token should report expires_in = 0");

        assert!(loaded.token_received_at.is_some(), "token_received_at must be set for rmcp to attempt refresh");

        assert_eq!(
            token.refresh_token().map(|t| t.secret().as_str()),
            Some("refresh"),
            "refresh token must be preserved for expired credentials"
        );
    }

    #[tokio::test]
    async fn unexpired_credential_sets_positive_expires_in() {
        let store: Arc<dyn OAuthCredentialStorage> = Arc::new(FakeOAuthCredentialStore::new());
        store.save_credential("server", unexpired_credential()).await.unwrap();

        let mcp_store = mcp_credential_store(store.clone(), "server".to_string());
        let loaded = CredentialStore::load(&mcp_store).await.unwrap().unwrap();
        let token = loaded.token_response.as_ref().unwrap();

        let expires_in = token.expires_in().expect("expires_in must be set for unexpired tokens with expiry info");
        assert!(expires_in > Duration::ZERO, "unexpired token should report positive expires_in, got {expires_in:?}");
        assert!(expires_in.as_secs() > 3500, "expires_in should be close to 1 hour");
    }

    #[tokio::test]
    async fn with_now_fn_overrides_time_source() {
        fn fake_now() -> Seconds {
            Seconds(1_000_000)
        }

        let store: Arc<dyn OAuthCredentialStorage> = Arc::new(FakeOAuthCredentialStore::new());
        store.save_credential("server", unexpired_credential()).await.unwrap();

        let mcp_store = mcp_credential_store(store.clone(), "server".to_string()).with_now_fn(fake_now);
        let loaded = CredentialStore::load(&mcp_store).await.unwrap().unwrap();

        assert_eq!(loaded.token_received_at, Some(1_000_000));
    }

    fn now_epoch_millis() -> u64 {
        let d = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        d.as_secs() * 1000 + u64::from(d.subsec_millis())
    }

    fn unexpired_credential() -> OAuthCredential {
        OAuthCredential {
            client_id: "client".to_string(),
            access_token: "access".to_string(),
            refresh_token: Some("refresh".to_string()),
            expires_at: Some(now_epoch_millis() + 3_600_000),
        }
    }

    fn expired_credential_with_refresh() -> OAuthCredential {
        OAuthCredential {
            client_id: "client".to_string(),
            access_token: "stale_access".to_string(),
            refresh_token: Some("refresh".to_string()),
            expires_at: Some(now_epoch_millis() - 600_000),
        }
    }
}
