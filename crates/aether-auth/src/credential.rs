use async_trait::async_trait;
use oauth2::basic::BasicClient;
use oauth2::reqwest::redirect::Policy;
use oauth2::{ClientId, RefreshToken, TokenResponse, TokenUrl};
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::OAuthError;

const TOKEN_EXPIRY_GRACE_PERIOD: Duration = Duration::from_mins(1);

/// Credential for an OAuth provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthCredential {
    pub client_id: String,
    pub access_token: String,
    pub refresh_token: Option<String>,
    /// Unix timestamp in milliseconds when the token expires.
    pub expires_at: Option<u64>,
    /// Scopes the authorization server granted with this token.
    #[serde(default)]
    pub granted_scopes: Vec<String>,
}

impl OAuthCredential {
    /// Build an `OAuthCredential` from an `OAuth2` token response.
    pub fn from_token_response<T: TokenResponse>(client_id: String, token_response: &T) -> Self {
        Self {
            client_id,
            access_token: token_response.access_token().secret().clone(),
            refresh_token: token_response.refresh_token().map(|token| token.secret().clone()),
            expires_at: expires_at_from_duration(token_response.expires_in()),
            granted_scopes: token_response
                .scopes()
                .map(|scopes| scopes.iter().map(|scope| scope.to_string()).collect())
                .unwrap_or_default(),
        }
    }

    /// Whether the access token is expired or expiring within the refresh skew.
    pub fn needs_refresh(&self) -> bool {
        self.expires_at.is_some_and(|at| {
            current_unix_time_millis() >= at.saturating_sub(duration_millis(TOKEN_EXPIRY_GRACE_PERIOD))
        })
    }

    /// Time remaining before the access token expires, if known and still in the future.
    pub fn expires_in(&self) -> Option<Duration> {
        self.expires_at.and_then(|expires_at| {
            let now = current_unix_time_millis();
            (expires_at > now).then(|| Duration::from_millis(expires_at - now))
        })
    }

    /// Exchange the refresh token for a new access token.
    ///
    /// Preserves the existing refresh token if the response doesn't include a rotated one.
    /// Returns `NoCredentials` if the credential has no refresh token to exchange.
    pub async fn refresh(self, token_url: &TokenUrl) -> Result<Self, OAuthError> {
        let old_refresh_token = self.refresh_token.clone().ok_or_else(|| {
            OAuthError::NoCredentials(
                "OAuth credential expired and no refresh token is available. Re-run OAuth login.".to_string(),
            )
        })?;

        let oauth_client = BasicClient::new(ClientId::new(self.client_id.clone())).set_token_uri(token_url.clone());
        let http_client = oauth_http_client()?;
        let token_response = oauth_client
            .exchange_refresh_token(&RefreshToken::new(old_refresh_token.clone()))
            .request_async(&http_client)
            .await
            .map_err(|e| OAuthError::TokenExchange(e.to_string()))?;

        let mut refreshed = Self::from_token_response(self.client_id, &token_response);
        if refreshed.refresh_token.is_none() {
            refreshed.refresh_token = Some(old_refresh_token);
        }
        Ok(refreshed)
    }
}

/// Trait for loading and saving OAuth credentials, keyed by provider ID or credential key.
///
/// Implementations include [`OsKeyringStore`](crate::OsKeyringStore) (OS keychain, feature `keyring`)
/// and the in-memory [`FakeOAuthCredentialStore`](crate::FakeOAuthCredentialStore) for tests.
#[async_trait]
pub trait OAuthCredentialStorage: Send + Sync {
    async fn load_credential(&self, key: &str) -> Result<Option<OAuthCredential>, OAuthError>;

    async fn save_credential(&self, key: &str, credential: OAuthCredential) -> Result<(), OAuthError>;

    async fn delete_credential(&self, key: &str) -> Result<(), OAuthError>;

    fn has_credential(&self, key: &str) -> bool;
}

fn expires_at_from_duration(duration: Option<Duration>) -> Option<u64> {
    duration.map(|duration| current_unix_time_millis().saturating_add(duration_millis(duration)))
}

pub fn oauth_http_client() -> Result<oauth2::reqwest::Client, OAuthError> {
    oauth2::reqwest::Client::builder()
        .redirect(Policy::none())
        .build()
        .map_err(|e| OAuthError::TokenExchange(format!("failed to build HTTP client: {e}")))
}

fn current_unix_time_millis() -> u64 {
    u64::try_from(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis())
        .unwrap_or(u64::MAX)
}

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_refresh_is_false_when_no_expiry() {
        assert!(!build_credential(None).needs_refresh());
    }

    #[test]
    fn needs_refresh_is_false_when_far_in_future() {
        assert!(!build_credential(Some(u64::MAX)).needs_refresh());
    }

    #[test]
    fn needs_refresh_is_true_when_past() {
        assert!(build_credential(Some(0)).needs_refresh());
    }

    #[test]
    fn needs_refresh_is_true_when_within_skew() {
        let cred = build_credential(expires_at_from_duration(Some(Duration::from_millis(59_999))));
        assert!(cred.needs_refresh());
    }

    #[test]
    fn expires_in_is_none_when_no_expiry() {
        assert!(build_credential(None).expires_in().is_none());
    }

    #[test]
    fn expires_in_is_none_when_already_past() {
        assert!(build_credential(Some(0)).expires_in().is_none());
    }

    #[test]
    fn expires_in_returns_remaining_duration_when_future() {
        let cred = build_credential(expires_at_from_duration(Some(Duration::from_hours(1))));
        let remaining = cred.expires_in().expect("expires_in should be Some for future expiry");
        assert!(remaining > Duration::from_mins(58));
        assert!(remaining <= Duration::from_hours(1));
    }

    fn build_credential(expires_at: Option<u64>) -> OAuthCredential {
        OAuthCredential {
            client_id: "client".to_string(),
            access_token: "access".to_string(),
            refresh_token: None,
            expires_at,
            granted_scopes: Vec::new(),
        }
    }
}
