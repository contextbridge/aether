use async_trait::async_trait;
use rmcp::transport::auth::{AuthError, CredentialStore, StoredCredentials};
use std::sync::Arc;

use crate::{OAuthCredentialStorage, OAuthError};

/// Per-server adapter that binds an [`OAuthCredentialStorage`] to a single MCP server id
/// and implements `rmcp::transport::auth::CredentialStore` by persisting rmcp's
/// [`StoredCredentials`] verbatim as a namespaced, opaque JSON secret.
///
/// rmcp owns the credential schema (tokens, granted scopes, issuer binding,
/// `token_received_at`), so nothing is translated in either direction.
#[derive(Clone)]
pub struct McpCredentialStore {
    server_id: String,
    store: Arc<dyn OAuthCredentialStorage>,
    expected_client_id: Option<String>,
}

impl McpCredentialStore {
    pub fn new(store: Arc<dyn OAuthCredentialStorage>, server_id: String) -> Self {
        Self { server_id, store, expected_client_id: None }
    }

    /// Only serve stored credentials issued to this client id; credentials for any
    /// other client are ignored on load (and replaced on the next successful flow).
    pub fn with_expected_client_id(mut self, client_id: Option<&str>) -> Self {
        self.expected_client_id = client_id.map(String::from);
        self
    }

    /// Load and filter persisted credentials.
    pub async fn load_stored(&self) -> Result<Option<StoredCredentials>, OAuthError> {
        self.store
            .load(&self.secret_key())
            .await?
            .map(serde_json::from_value::<StoredCredentials>)
            .transpose()
            .map_err(|error| OAuthError::CredentialStore(format!("invalid MCP credential: {error}")))
            .map(|credentials| {
                credentials.filter(|credential| {
                    self.expected_client_id.as_deref().is_none_or(|expected| credential.client_id == expected)
                })
            })
    }

    fn secret_key(&self) -> String {
        format!("mcp:{}", self.server_id)
    }
}

#[async_trait]
impl CredentialStore for McpCredentialStore {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        self.load_stored().await.map_err(|e| internal_err(&e))
    }

    async fn save(&self, credentials: StoredCredentials) -> Result<(), AuthError> {
        let value = serde_json::to_value(&credentials)
            .map_err(|e| AuthError::InternalError(format!("failed to serialize credentials: {e}")))?;
        self.store.save(&self.secret_key(), value).await.map_err(|e| internal_err(&e))
    }

    async fn clear(&self) -> Result<(), AuthError> {
        self.store.delete(&self.secret_key()).await.map_err(|e| internal_err(&e))
    }
}

fn internal_err(error: &OAuthError) -> AuthError {
    AuthError::InternalError(error.to_string())
}

#[cfg(test)]
mod tests {
    use oauth2::basic::BasicTokenType;
    use oauth2::{AccessToken, RefreshToken, TokenResponse};
    use rmcp::transport::auth::{OAuthTokenResponse, VendorExtraTokenFields};
    use std::time::Duration;

    use super::*;
    use crate::FakeOAuthCredentialStore;

    fn stored_credentials(client_id: &str) -> StoredCredentials {
        let mut token_response = OAuthTokenResponse::new(
            AccessToken::new("access".to_string()),
            BasicTokenType::Bearer,
            VendorExtraTokenFields::default(),
        );
        token_response.set_refresh_token(Some(RefreshToken::new("refresh".to_string())));
        token_response.set_expires_in(Some(&Duration::from_hours(1)));
        StoredCredentials::new(client_id.to_string(), Some(token_response), vec!["read".to_string()], Some(2_000_000))
            .with_issuer(Some("https://issuer.example".to_string()))
    }

    fn mcp_store(store: Arc<FakeOAuthCredentialStore>) -> McpCredentialStore {
        McpCredentialStore::new(store, "server".to_string())
    }

    #[tokio::test]
    async fn save_then_load_round_trips_stored_credentials_verbatim() {
        let store = Arc::new(FakeOAuthCredentialStore::new());
        let mcp_store = mcp_store(store.clone());

        CredentialStore::save(&mcp_store, stored_credentials("client")).await.unwrap();

        let loaded = CredentialStore::load(&mcp_store).await.unwrap().unwrap();
        assert_eq!(loaded.client_id, "client");
        assert_eq!(loaded.granted_scopes, vec!["read"]);
        assert_eq!(loaded.token_received_at, Some(2_000_000));
        assert_eq!(loaded.issuer.as_deref(), Some("https://issuer.example"));
        let token = loaded.token_response.unwrap();
        assert_eq!(token.access_token().secret(), "access");
        assert_eq!(token.refresh_token().map(|t| t.secret().as_str()), Some("refresh"));
        assert_eq!(token.expires_in(), Some(Duration::from_hours(1)));
    }

    #[tokio::test]
    async fn tokenless_registration_round_trips() {
        let store = Arc::new(FakeOAuthCredentialStore::new());
        let mcp_store = mcp_store(store.clone());
        let registration =
            StoredCredentials::new("https://client.example/metadata.json".to_string(), None, Vec::new(), None)
                .with_issuer(Some("https://new-issuer.example".to_string()));

        CredentialStore::save(&mcp_store, registration).await.unwrap();

        let loaded = CredentialStore::load(&mcp_store).await.unwrap().expect("portable client registration");
        assert_eq!(loaded.client_id, "https://client.example/metadata.json");
        assert_eq!(loaded.issuer.as_deref(), Some("https://new-issuer.example"));
        assert!(loaded.token_response.is_none());
    }

    #[tokio::test]
    async fn load_filters_credentials_for_unexpected_client() {
        let store = Arc::new(FakeOAuthCredentialStore::new());
        CredentialStore::save(&mcp_store(store.clone()), stored_credentials("client")).await.unwrap();

        let filtered = mcp_store(store.clone()).with_expected_client_id(Some("other-client"));

        assert!(CredentialStore::load(&filtered).await.unwrap().is_none());
        assert!(store.load("mcp:server").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn load_serves_credentials_for_expected_client() {
        let store = Arc::new(FakeOAuthCredentialStore::new());
        CredentialStore::save(&mcp_store(store.clone()), stored_credentials("client")).await.unwrap();

        let matching = mcp_store(store).with_expected_client_id(Some("client"));

        assert_eq!(CredentialStore::load(&matching).await.unwrap().unwrap().client_id, "client");
    }

    #[tokio::test]
    async fn load_rejects_undecodable_value() {
        let store = Arc::new(FakeOAuthCredentialStore::new().with_value("mcp:server", serde_json::json!("invalid")));

        assert!(CredentialStore::load(&mcp_store(store)).await.is_err());
    }

    #[tokio::test]
    async fn clear_removes_credential() {
        let store = Arc::new(FakeOAuthCredentialStore::new());
        let mcp_store = mcp_store(store.clone());
        CredentialStore::save(&mcp_store, stored_credentials("client")).await.unwrap();

        CredentialStore::clear(&mcp_store).await.unwrap();

        assert!(store.load("mcp:server").await.unwrap().is_none());
    }
}
