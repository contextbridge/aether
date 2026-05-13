use crate::error::OAuthError;
use futures::future::BoxFuture;

/// OAuth callback data containing both the authorization code and state (CSRF token)
#[derive(Debug, Clone)]
pub struct OAuthCallback {
    pub code: String,
    pub state: String,
}

/// Trait that consuming applications implement to handle OAuth UI/UX.
///
/// Uses `BoxFuture` instead of `async fn` to support `dyn OAuthHandler`
/// (required for `Arc<dyn OAuthHandler>` in `McpManager`).
pub trait OAuthHandler: Send + Sync {
    /// The redirect URI the OAuth provider should send the user back to,
    /// e.g. `http://127.0.0.1:<port>/oauth2callback`.
    fn redirect_uri(&self) -> &str;

    /// Called when user needs to authorize. App should open browser to `auth_url`
    /// and return the authorization code and state (CSRF token) from the callback.
    fn authorize(&self, auth_url: &str) -> BoxFuture<'_, Result<OAuthCallback, OAuthError>>;
}
