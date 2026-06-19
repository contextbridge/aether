use crate::LlmError;
use aether_auth::{
    BrowserOAuthHandler, OAuthCredential, OAuthCredentialStorage, OAuthError, OAuthHandler, oauth_http_client,
};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use oauth2::basic::BasicClient;
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

    let http_client = oauth_http_client()?;

    let token_response = oauth_client
        .exchange_code(AuthorizationCode::new(callback.code))
        .set_pkce_verifier(pkce_verifier)
        .request_async(&http_client)
        .await
        .map_err(|e| OAuthError::TokenExchange(e.to_string()))?;

    let credential = OAuthCredential::from_token_response(CLIENT_ID.to_string(), &token_response);
    store.save_credential(super::PROVIDER_ID, credential).await?;

    Ok(())
}

/// In-memory cache of the most recently validated credential and its derived account ID.
struct CachedToken {
    credential: OAuthCredential,
    account_id: String,
}

/// Manages OAuth tokens for the Codex backend API.
///
/// Holds an `Arc<dyn OAuthCredentialStorage>` so callers can swap in keyring-backed,
/// file-backed, or in-memory stores without changing this type.
pub struct CodexTokenManager {
    store: Arc<dyn OAuthCredentialStorage>,
    credential_key: String,
    token_url: TokenUrl,
    cached: Mutex<Option<CachedToken>>,
}

impl CodexTokenManager {
    pub fn new(store: Arc<dyn OAuthCredentialStorage>, credential_key: &str) -> Self {
        Self::new_with_token_url(
            store,
            credential_key,
            TokenUrl::new(TOKEN_URL.to_string()).expect("hardcoded Codex token URL is valid"),
        )
    }

    fn new_with_token_url(store: Arc<dyn OAuthCredentialStorage>, credential_key: &str, token_url: TokenUrl) -> Self {
        Self { store, credential_key: credential_key.to_string(), token_url, cached: Mutex::new(None) }
    }

    /// Get a valid access token and account ID.
    ///
    /// Returns `(access_token, account_id)`. The account ID is extracted from
    /// the JWT's `https://api.openai.com/auth` claim field `chatgpt_account_id`.
    pub async fn get_valid_token(&self) -> Result<(String, String), LlmError> {
        let mut cache = self.cached.lock().await;
        if let Some(cached) = cache.as_ref()
            && !cached.credential.needs_refresh()
        {
            return Ok((cached.credential.access_token.clone(), cached.account_id.clone()));
        }

        let credential = self.load_or_refresh().await?;
        let account_id = extract_account_id(&credential.access_token)?;
        let access_token = credential.access_token.clone();
        *cache = Some(CachedToken { credential, account_id: account_id.clone() });
        Ok((access_token, account_id))
    }

    async fn load_or_refresh(&self) -> Result<OAuthCredential, LlmError> {
        let stored = self.store.load_credential(&self.credential_key).await?.ok_or_else(|| {
            OAuthError::NoCredentials(
                "No Codex OAuth credentials found. Run `aether` and select a codex model to trigger OAuth login."
                    .to_string(),
            )
        })?;

        if !stored.needs_refresh() {
            return Ok(stored);
        }

        let refreshed = stored.refresh(&self.token_url).await?;
        self.store.save_credential(&self.credential_key, refreshed.clone()).await?;
        Ok(refreshed)
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
    use aether_auth::{FakeOAuthCredentialStore, OAuthCredential};
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::extract::State;
    use axum::http::{HeaderMap, Method, Request, StatusCode};
    use axum::response::IntoResponse;
    use axum::routing::post;
    use std::collections::HashMap;
    use tokio::net::TcpListener;
    use tokio::sync::{Mutex as TokioMutex, oneshot};

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

    #[tokio::test]
    async fn codex_token_manager_refreshes_expired_credential() {
        let new_access_token = test_jwt_for_account("acct_new");
        let endpoint = FakeTokenEndpoint::start(TokenEndpointResponse::success(&new_access_token, None)).await;
        let store = Arc::new(
            FakeOAuthCredentialStore::new()
                .with_credential("codex", expired_credential("old-access", Some("refresh-old"))),
        );

        let manager = CodexTokenManager::new_with_token_url(store.clone(), "codex", endpoint.url.clone());
        let (access_token, account_id) = manager.get_valid_token().await.unwrap();
        let request = endpoint.request.await.expect("token endpoint request");
        let saved = store.load_credential("codex").await.unwrap().unwrap();

        assert_eq!(access_token, new_access_token);
        assert_eq!(account_id, "acct_new");
        assert_eq!(saved.access_token, new_access_token);
        assert_eq!(saved.refresh_token.as_deref(), Some("refresh-old"));
        assert_eq!(request.method, Method::POST);
        assert_eq!(request.path, "/oauth/token");
        assert_eq!(request.form.get("grant_type").map(String::as_str), Some("refresh_token"));
        assert_eq!(request.form.get("refresh_token").map(String::as_str), Some("refresh-old"));
        assert_eq!(request.form.get("client_id").map(String::as_str), Some(CLIENT_ID));
        assert!(request.headers.get("accept").is_some());
    }

    #[tokio::test]
    async fn codex_token_manager_saves_rotated_refresh_token() {
        let new_access_token = test_jwt_for_account("acct_new");
        let endpoint =
            FakeTokenEndpoint::start(TokenEndpointResponse::success(&new_access_token, Some("refresh-new"))).await;
        let store = Arc::new(
            FakeOAuthCredentialStore::new()
                .with_credential("codex", expired_credential("old-access", Some("refresh-old"))),
        );
        let manager = CodexTokenManager::new_with_token_url(store.clone(), "codex", endpoint.url.clone());
        manager.get_valid_token().await.unwrap();
        let saved = store.load_credential("codex").await.unwrap().unwrap();

        assert_eq!(saved.access_token, new_access_token);
        assert_eq!(saved.refresh_token.as_deref(), Some("refresh-new"));
    }

    #[tokio::test]
    async fn codex_token_manager_uses_unexpired_credential_without_refresh() {
        let access_token = test_jwt_for_account("acct_existing");
        let store = Arc::new(FakeOAuthCredentialStore::new().with_credential(
            "codex",
            OAuthCredential {
                client_id: CLIENT_ID.to_string(),
                access_token: access_token.clone(),
                refresh_token: Some("refresh-old".to_string()),
                expires_at: Some(u64::MAX),
                granted_scopes: Vec::new(),
            },
        ));

        let manager = CodexTokenManager::new_with_token_url(
            store,
            "codex",
            TokenUrl::new("http://127.0.0.1:9/oauth/token".to_string()).unwrap(),
        );

        let (returned_token, account_id) = manager.get_valid_token().await.unwrap();
        assert_eq!(returned_token, access_token);
        assert_eq!(account_id, "acct_existing");
    }

    #[tokio::test]
    async fn codex_token_manager_errors_when_credential_is_missing() {
        let store = Arc::new(FakeOAuthCredentialStore::new());
        let manager = CodexTokenManager::new_with_token_url(
            store,
            "codex",
            TokenUrl::new("http://127.0.0.1:9/oauth/token".to_string()).unwrap(),
        );

        let error = manager.get_valid_token().await.unwrap_err();
        assert!(error.to_string().contains("No Codex OAuth credentials found"));
        assert!(error.to_string().contains("select a codex model"));
    }

    #[tokio::test]
    async fn codex_token_manager_errors_when_expired_without_refresh_token() {
        let original = expired_credential("old-access", None);
        let store = Arc::new(FakeOAuthCredentialStore::new().with_credential("codex", original.clone()));
        let manager = CodexTokenManager::new_with_token_url(
            store.clone(),
            "codex",
            TokenUrl::new("http://127.0.0.1:9/oauth/token".to_string()).unwrap(),
        );

        let error = manager.get_valid_token().await.unwrap_err();
        let saved = store.load_credential("codex").await.unwrap().unwrap();

        assert!(error.to_string().contains("Re-run OAuth login"));
        assert_eq!(saved.access_token, original.access_token);
        assert_eq!(saved.refresh_token, original.refresh_token);
    }

    #[tokio::test]
    async fn codex_token_manager_does_not_overwrite_credential_when_refresh_fails() {
        let endpoint = FakeTokenEndpoint::start(TokenEndpointResponse::failure()).await;
        let original = expired_credential("old-access", Some("refresh-old"));
        let store = Arc::new(FakeOAuthCredentialStore::new().with_credential("codex", original.clone()));
        let manager = CodexTokenManager::new_with_token_url(store.clone(), "codex", endpoint.url.clone());

        let result = manager.get_valid_token().await;
        let saved = store.load_credential("codex").await.unwrap().unwrap();

        assert!(result.is_err());
        assert_eq!(saved.access_token, original.access_token);
        assert_eq!(saved.refresh_token, original.refresh_token);
    }

    struct FakeTokenEndpoint {
        url: TokenUrl,
        request: oneshot::Receiver<CapturedTokenRequest>,
    }

    struct CapturedTokenRequest {
        method: Method,
        path: String,
        headers: HeaderMap,
        form: HashMap<String, String>,
    }

    #[derive(Clone)]
    struct FakeTokenState {
        response: TokenEndpointResponse,
        request_tx: Arc<TokioMutex<Option<oneshot::Sender<CapturedTokenRequest>>>>,
        shutdown_tx: Arc<TokioMutex<Option<oneshot::Sender<()>>>>,
    }

    #[derive(Clone)]
    struct TokenEndpointResponse {
        status: StatusCode,
        body: serde_json::Value,
    }

    impl TokenEndpointResponse {
        fn success(access_token: &str, refresh_token: Option<&str>) -> Self {
            let mut body = serde_json::json!({
                "access_token": access_token,
                "token_type": "Bearer",
                "expires_in": 3600
            });
            if let Some(refresh_token) = refresh_token {
                body["refresh_token"] = serde_json::Value::String(refresh_token.to_string());
            }
            Self { status: StatusCode::OK, body }
        }

        fn failure() -> Self {
            Self { status: StatusCode::BAD_REQUEST, body: serde_json::json!({ "error": "invalid_grant" }) }
        }
    }

    impl FakeTokenEndpoint {
        async fn start(response: TokenEndpointResponse) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind fake token endpoint");
            let url = TokenUrl::new(format!(
                "http://{}/oauth/token",
                listener.local_addr().expect("fake token endpoint address")
            ))
            .expect("fake token endpoint URL is valid");
            let (request_tx, request) = oneshot::channel();
            let (shutdown_tx, shutdown) = oneshot::channel();
            let state = FakeTokenState {
                response,
                request_tx: Arc::new(TokioMutex::new(Some(request_tx))),
                shutdown_tx: Arc::new(TokioMutex::new(Some(shutdown_tx))),
            };
            let app = Router::new().route("/oauth/token", post(capture_token_request)).with_state(state);
            tokio::spawn(async move {
                axum::serve(listener, app)
                    .with_graceful_shutdown(async {
                        let _ = shutdown.await;
                    })
                    .await
                    .expect("serve fake token endpoint");
            });
            Self { url, request }
        }
    }

    async fn capture_token_request(State(state): State<FakeTokenState>, request: Request<Body>) -> impl IntoResponse {
        let (parts, body) = request.into_parts();
        let body = to_bytes(body, usize::MAX).await.expect("read token request body");
        let form = url::form_urlencoded::parse(&body).into_owned().collect();
        if let Some(tx) = state.request_tx.lock().await.take() {
            let _ = tx.send(CapturedTokenRequest {
                method: parts.method,
                path: parts.uri.path().to_string(),
                headers: parts.headers,
                form,
            });
        }
        if let Some(tx) = state.shutdown_tx.lock().await.take() {
            let _ = tx.send(());
        }
        (state.response.status, axum::Json(state.response.body))
    }

    fn expired_credential(access_token: &str, refresh_token: Option<&str>) -> OAuthCredential {
        OAuthCredential {
            client_id: CLIENT_ID.to_string(),
            access_token: access_token.to_string(),
            refresh_token: refresh_token.map(str::to_string),
            expires_at: Some(0),
            granted_scopes: Vec::new(),
        }
    }

    fn test_jwt_for_account(account_id: &str) -> String {
        make_test_jwt(&serde_json::json!({
            "https://api.openai.com/auth": {
                "chatgpt_account_id": account_id
            }
        }))
    }
}
