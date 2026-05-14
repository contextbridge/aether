use async_trait::async_trait;
use oauth2::{AccessToken, RefreshToken};
use rmcp::transport::auth::{
    AuthError, CredentialStore, OAuthTokenResponse, StoredCredentials, VendorExtraTokenFields,
};
use std::sync::Arc;

use crate::{OAuthCredential, OAuthCredentialStorage};

/// Per-server adapter that binds an [`OAuthCredentialStorage`] to a single MCP server id
/// and implements `rmcp::transport::auth::CredentialStore`.
#[derive(Clone)]
pub struct McpCredentialStore {
    server_id: String,
    store: Arc<dyn OAuthCredentialStorage>,
}

/// Build an `McpCredentialStore` bound to a specific MCP server id.
pub fn mcp_credential_store(store: Arc<dyn OAuthCredentialStorage>, server_id: String) -> McpCredentialStore {
    McpCredentialStore { server_id, store }
}

#[async_trait]
impl CredentialStore for McpCredentialStore {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        let cred =
            self.store.load_credential(&self.server_id).await.map_err(|e| AuthError::InternalError(e.to_string()))?;

        Ok(cred.map(|c| {
            let token_response = build_token_response(&c);
            build_stored_credentials(&c.client_id, Some(&token_response))
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

/// Construct a `StoredCredentials` via serde deserialization.
///
/// The upstream struct is `#[non_exhaustive]` with no constructor, so this is
/// the only way to build one from outside the crate.
fn build_stored_credentials(client_id: &str, token_response: Option<&OAuthTokenResponse>) -> StoredCredentials {
    serde_json::from_value(serde_json::json!({
        "client_id": client_id,
        "token_response": token_response,
    }))
    .expect("StoredCredentials deserialization from known-good fields cannot fail")
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
        let stored = build_stored_credentials(&cred.client_id, Some(&token_response));

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
        let stored = build_stored_credentials(
            "client",
            Some(&OAuthTokenResponse::new(
                AccessToken::new("token".to_string()),
                BasicTokenType::Bearer,
                VendorExtraTokenFields::default(),
            )),
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
}
