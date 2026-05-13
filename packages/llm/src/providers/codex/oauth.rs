use crate::LlmError;
use aether_auth::{BrowserOAuthHandler, OAuthCredential, OAuthCredentialStorage, OAuthError, OAuthHandler};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use oauth2::TokenResponse;
use oauth2::basic::BasicClient;
use oauth2::reqwest::redirect::Policy;
use oauth2::{AuthUrl, AuthorizationCode, ClientId, PkceCodeChallenge, RedirectUrl, TokenUrl};
use std::sync::Arc;
use tokio::sync::Mutex;
use url::Url;

const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";
const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
const SCOPE: &str = "openid profile email offline_access";

/// Run the full Codex OAuth flow: open browser, capture callback, exchange token, save credentials.
///
pub async fn perform_codex_oauth_flow(store: &dyn OAuthCredentialStorage) -> Result<(), LlmError> {
    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
    let state = generate_random_state();

    let auth_url = Url::parse_with_params(
        AUTHORIZE_URL,
        &[
            ("response_type", "code"),
            ("client_id", CLIENT_ID),
            ("redirect_uri", REDIRECT_URI),
            ("scope", SCOPE),
            ("code_challenge", pkce_challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("state", &state),
            ("id_token_add_organizations", "true"),
            ("codex_cli_simplified_flow", "true"),
            ("originator", "codex_cli_rs"),
        ],
    )
    .map_err(|e| OAuthError::TokenExchange(format!("Failed to build auth URL: {e}")))?;

    // Port 1455 is hardcoded because the Codex API has a fixed redirect URI
    // (http://localhost:1455/auth/callback) registered with OpenAI's OAuth server.
    let handler = BrowserOAuthHandler::with_redirect_uri(REDIRECT_URI, 1455)?;
    let callback = handler.authorize(auth_url.as_str()).await?;

    if callback.state != state {
        return Err(OAuthError::StateMismatch.into());
    }

    let oauth_client = BasicClient::new(ClientId::new(CLIENT_ID.to_string()))
        .set_auth_uri(
            AuthUrl::new(AUTHORIZE_URL.to_string())
                .map_err(|e| OAuthError::TokenExchange(format!("invalid auth URL: {e}")))?,
        )
        .set_token_uri(
            TokenUrl::new(TOKEN_URL.to_string())
                .map_err(|e| OAuthError::TokenExchange(format!("invalid token URL: {e}")))?,
        )
        .set_redirect_uri(
            RedirectUrl::new(REDIRECT_URI.to_string())
                .map_err(|e| OAuthError::TokenExchange(format!("invalid redirect URI: {e}")))?,
        );

    let http_client = oauth2::reqwest::Client::builder()
        .redirect(Policy::none())
        .build()
        .map_err(|e| OAuthError::TokenExchange(format!("failed to build HTTP client: {e}")))?;

    let token_response = oauth_client
        .exchange_code(AuthorizationCode::new(callback.code))
        .set_pkce_verifier(pkce_verifier)
        .request_async(&http_client)
        .await
        .map_err(|e| OAuthError::TokenExchange(e.to_string()))?;

    let expires_at = token_response.expires_in().map(|duration| {
        let now_ms = u64::try_from(
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis(),
        )
        .unwrap_or(u64::MAX);
        let duration_ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
        now_ms.saturating_add(duration_ms)
    });

    let credential = OAuthCredential {
        client_id: CLIENT_ID.to_string(),
        access_token: token_response.access_token().secret().clone(),
        refresh_token: token_response.refresh_token().map(|t| t.secret().clone()),
        expires_at,
    };

    store.save_credential(super::PROVIDER_ID, credential).await?;

    Ok(())
}

/// Cached token with optional expiry.
struct CachedToken {
    access_token: String,
    account_id: String,
    /// Unix timestamp in milliseconds when the token expires
    expires_at: Option<u64>,
}

impl CachedToken {
    fn is_expired(&self) -> bool {
        let Some(expires_at) = self.expires_at else {
            return false;
        };
        let now_ms = u64::try_from(
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis(),
        )
        .unwrap_or(u64::MAX);
        now_ms >= expires_at
    }
}

/// Manages OAuth tokens for the Codex backend API.
///
/// Holds an `Arc<dyn OAuthCredentialStorage>` so callers can swap in keyring-backed,
/// file-backed, or in-memory stores without changing this type.
pub struct CodexTokenManager {
    store: Arc<dyn OAuthCredentialStorage>,
    credential_key: String,
    cached: Mutex<Option<CachedToken>>,
}

impl CodexTokenManager {
    pub fn new(store: Arc<dyn OAuthCredentialStorage>, credential_key: &str) -> Self {
        Self { store, credential_key: credential_key.to_string(), cached: Mutex::new(None) }
    }

    /// Get a valid access token and account ID.
    ///
    /// Returns `(access_token, account_id)`. The account ID is extracted from
    /// the JWT's `https://api.openai.com/auth` claim field `chatgpt_account_id`.
    pub async fn get_valid_token(&self) -> Result<(String, String), LlmError> {
        // Check cache first — return if present and not expired
        {
            let guard = self.cached.lock().await;
            if let Some(cached) = guard.as_ref()
                && !cached.is_expired()
            {
                return Ok((cached.access_token.clone(), cached.account_id.clone()));
            }
        }

        let credential = self
            .store
            .load_credential(&self.credential_key)
            .await
            .map_err(|e| OAuthError::NoCredentials(e.to_string()))?
            .ok_or_else(|| {
                OAuthError::NoCredentials(
                    "No Codex OAuth credentials found. Run `aether` and select a codex model to trigger OAuth login."
                        .to_string(),
                )
            })?;

        let account_id = extract_account_id(&credential.access_token)?;

        let cached = CachedToken {
            access_token: credential.access_token.clone(),
            account_id: account_id.clone(),
            expires_at: credential.expires_at,
        };
        *self.cached.lock().await = Some(cached);

        Ok((credential.access_token, account_id))
    }

    /// Clear the cached token (e.g. after a 401 response)
    pub async fn clear_cache(&self) {
        *self.cached.lock().await = None;
    }
}

/// Extract the account ID from a JWT access token.
///
/// The JWT payload contains a claim at `https://api.openai.com/auth`
/// with a `chatgpt_account_id` field.
pub fn extract_account_id(access_token: &str) -> Result<String, LlmError> {
    let parts: Vec<&str> = access_token.split('.').collect();
    if parts.len() != 3 {
        return Err(OAuthError::InvalidJwt("expected 3 dot-separated parts".to_string()).into());
    }

    let decoded = URL_SAFE_NO_PAD
        .decode(parts[1])
        .map_err(|e| OAuthError::InvalidJwt(format!("failed to decode payload: {e}")))?;

    let payload: serde_json::Value = serde_json::from_slice(&decoded)
        .map_err(|e| OAuthError::InvalidJwt(format!("failed to parse payload: {e}")))?;

    let account_id = payload
        .get("https://api.openai.com/auth")
        .and_then(|auth| auth.get("chatgpt_account_id"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| OAuthError::InvalidJwt("missing chatgpt_account_id in token".to_string()))?;

    Ok(account_id.to_string())
}

fn generate_random_state() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a test JWT with a given payload
    fn make_test_jwt(payload: &serde_json::Value) -> String {
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256","typ":"JWT"}"#);
        let payload_json = serde_json::to_string(payload).unwrap();
        let payload_b64url = URL_SAFE_NO_PAD.encode(payload_json.as_bytes());
        format!("{header}.{payload_b64url}.fake_signature")
    }

    #[test]
    fn extract_account_id_from_valid_jwt() {
        let payload = serde_json::json!({
            "sub": "user_123",
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "acct_abc123"
            }
        });

        let jwt = make_test_jwt(&payload);
        let account_id = extract_account_id(&jwt).unwrap();
        assert_eq!(account_id, "acct_abc123");
    }

    #[test]
    fn extract_account_id_missing_claim() {
        let payload = serde_json::json!({
            "sub": "user_123"
        });

        let jwt = make_test_jwt(&payload);
        let result = extract_account_id(&jwt);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("chatgpt_account_id"));
    }

    #[test]
    fn extract_account_id_invalid_jwt_format() {
        let result = extract_account_id("not.a.valid.jwt.too.many.parts");
        assert!(result.is_err());

        let result = extract_account_id("toofewparts");
        assert!(result.is_err());
    }

    #[test]
    fn extract_account_id_invalid_base64() {
        let result = extract_account_id("header.!!!invalid!!!.signature");
        assert!(result.is_err());
    }

    #[test]
    fn auth_url_is_well_formed() {
        let (pkce_challenge, _) = PkceCodeChallenge::new_random_sha256();
        let state = "test-state";

        let auth_url = Url::parse_with_params(
            AUTHORIZE_URL,
            &[
                ("response_type", "code"),
                ("client_id", CLIENT_ID),
                ("redirect_uri", REDIRECT_URI),
                ("scope", SCOPE),
                ("code_challenge", pkce_challenge.as_str()),
                ("code_challenge_method", "S256"),
                ("state", state),
                ("id_token_add_organizations", "true"),
                ("codex_cli_simplified_flow", "true"),
                ("originator", "codex_cli_rs"),
            ],
        )
        .unwrap();

        let url_str = auth_url.as_str();
        assert!(url_str.starts_with(AUTHORIZE_URL));
        assert!(url_str.contains("client_id="));
        assert!(url_str.contains("redirect_uri="));
        assert!(url_str.contains("scope="));
        assert!(url_str.contains("code_challenge="));
        assert!(url_str.contains("state=test-state"));
    }

    #[test]
    fn generate_random_state_is_valid_uuid() {
        let state = generate_random_state();
        assert!(!state.is_empty());
        assert!(uuid::Uuid::parse_str(&state).is_ok());
    }

    #[test]
    fn oauth_constants_are_valid() {
        assert!(AUTHORIZE_URL.starts_with("https://"));
        assert!(TOKEN_URL.starts_with("https://"));
        assert!(REDIRECT_URI.starts_with("http://localhost:"));
        assert!(SCOPE.contains("openid"));
    }

    #[test]
    fn cached_token_not_expired_when_no_expiry() {
        let token = CachedToken { access_token: "tok".to_string(), account_id: "acct".to_string(), expires_at: None };
        assert!(!token.is_expired());
    }

    #[test]
    fn cached_token_not_expired_when_future() {
        let future_ms =
            u64::try_from(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis())
                .unwrap()
                + 3_600_000; // 1 hour from now
        let token = CachedToken {
            access_token: "tok".to_string(),
            account_id: "acct".to_string(),
            expires_at: Some(future_ms),
        };
        assert!(!token.is_expired());
    }

    #[test]
    fn cached_token_expired_when_past() {
        let token = CachedToken {
            access_token: "tok".to_string(),
            account_id: "acct".to_string(),
            expires_at: Some(1000), // way in the past
        };
        assert!(token.is_expired());
    }
}
