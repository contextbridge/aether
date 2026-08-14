use crate::error::OAuthError;
use futures::future::BoxFuture;

/// UI boundary for interactive OAuth authorization.
pub trait OAuthHandler: Send + Sync {
    /// The redirect URI the OAuth provider should send the user back to.
    fn redirect_uri(&self) -> &str;

    /// Present `auth_url` and return the absolute callback URL.
    fn authorize(&self, auth_url: &str) -> BoxFuture<'_, Result<String, OAuthError>>;
}
