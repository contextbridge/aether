use super::credential_store::McpCredentialStore;
use crate::{OAuthCredentialStorage, OAuthError, OAuthHandler};
use rmcp::transport::auth::{AuthClient, AuthorizationManager, OAuthState};
use std::sync::Arc;

const OAUTH_CLIENT_NAME: &str = "Aether MCP Client";

/// Returns `Ok(Some(manager))` if credentials were found and initialized successfully,
/// `Ok(None)` if no stored credentials exist, or `Err` on failure.
pub async fn create_auth_manager_from_store(
    server_id: &str,
    base_url: &str,
    store: Arc<dyn OAuthCredentialStorage>,
) -> Result<Option<AuthorizationManager>, OAuthError> {
    let credential_store = McpCredentialStore::new(store, server_id.to_string());

    let mut auth_manager = AuthorizationManager::new(base_url).await.map_err(|e| OAuthError::Rmcp(e.to_string()))?;
    auth_manager.set_credential_store(credential_store);

    if auth_manager.initialize_from_store().await.map_err(|e| OAuthError::Rmcp(e.to_string()))? {
        Ok(Some(auth_manager))
    } else {
        Ok(None)
    }
}

/// Run a full OAuth authorization flow against an MCP server.
///
/// Creates the `OAuthState`, starts authorization using the handler's redirect URI,
/// opens the browser (via the handler), handles the callback, and returns an `AuthClient`
/// ready for authenticated HTTP transport. When `store` is `Some`, the resulting tokens
/// are persisted via the store; when `None`, tokens live only for the lifetime of the
/// returned `AuthClient`.
pub async fn perform_oauth_flow(
    server_id: &str,
    base_url: &str,
    handler: &dyn OAuthHandler,
    store: Option<Arc<dyn OAuthCredentialStorage>>,
) -> Result<AuthClient<reqwest::Client>, OAuthError> {
    let mut oauth_state =
        OAuthState::new(base_url, None).await.map_err(|e| OAuthError::Rmcp(format!("OAuth init failed: {e}")))?;

    if let Some(store) = store {
        let credential_store = McpCredentialStore::new(store, server_id.to_string());
        if let OAuthState::Unauthorized(ref mut manager) = oauth_state {
            manager.set_credential_store(credential_store);
        }
    }

    oauth_state
        .start_authorization(&[], handler.redirect_uri(), Some(OAUTH_CLIENT_NAME))
        .await
        .map_err(|e| OAuthError::Rmcp(format!("start_authorization failed: {e}")))?;

    let auth_url = oauth_state
        .get_authorization_url()
        .await
        .map_err(|e| OAuthError::Rmcp(format!("get_authorization_url failed: {e}")))?;

    // Some authorization servers (e.g. Sentry) bake `resource` into their
    // authorization_endpoint metadata. rmcp then adds its own `resource` param,
    // producing a duplicate that the server rejects with "invalid_target".
    let auth_url = dedupe_query_params(&auth_url);

    let callback = handler.authorize(&auth_url).await?;

    oauth_state
        .handle_callback(&callback.code, &callback.state)
        .await
        .map_err(|e| OAuthError::Rmcp(format!("handle_callback failed: {e}")))?;

    let auth_manager = oauth_state
        .into_authorization_manager()
        .ok_or_else(|| OAuthError::Rmcp("OAuth flow did not produce an AuthorizationManager".into()))?;

    Ok(AuthClient::new(reqwest::Client::default(), auth_manager))
}

fn dedupe_query_params(url_str: &str) -> String {
    let Ok(mut url) = url::Url::parse(url_str) else {
        return url_str.to_string();
    };
    let pairs: std::collections::HashMap<String, String> =
        url.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
    url.query_pairs_mut().clear().extend_pairs(&pairs);
    url.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedupes_duplicate_resource_param() {
        let input = "https://example.com/oauth/authorize?resource=https%3A%2F%2Fa%2Fb&response_type=code&client_id=x&resource=https%3A%2F%2Fa%2Fb";
        let out = dedupe_query_params(input);
        let url = url::Url::parse(&out).unwrap();
        let resources: Vec<_> = url.query_pairs().filter(|(k, _)| k == "resource").collect();
        assert_eq!(resources.len(), 1);
        assert_eq!(resources[0].1, "https://a/b");
        assert!(url.query_pairs().any(|(k, _)| k == "response_type"));
        assert!(url.query_pairs().any(|(k, _)| k == "client_id"));
    }

    #[test]
    fn preserves_unique_params() {
        let input = "https://example.com/?resource=x&other=y";
        let out = dedupe_query_params(input);
        let url = url::Url::parse(&out).unwrap();
        let pairs: Vec<_> = url.query_pairs().collect();
        assert_eq!(pairs.len(), 2);
    }
}
