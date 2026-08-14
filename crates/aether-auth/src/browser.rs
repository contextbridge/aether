use crate::error::OAuthError;
use crate::handler::OAuthHandler;
use futures::future::BoxFuture;
use std::process::Command;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::time::timeout;

/// Default `OAuthHandler` that opens the system browser and listens
/// for the OAuth callback on a dynamically-assigned local port.
pub struct BrowserOAuthHandler {
    listener: TcpListener,
    redirect_uri: String,
}

impl BrowserOAuthHandler {
    pub fn new() -> Result<Self, std::io::Error> {
        let std_listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let port = std_listener.local_addr()?.port();
        std_listener.set_nonblocking(true)?;
        let listener = TcpListener::from_std(std_listener)?;
        Ok(Self { listener, redirect_uri: format!("http://127.0.0.1:{port}/oauth2callback") })
    }

    /// Create a handler bound to a specific port with a custom redirect URI.
    pub fn with_redirect_uri(redirect_uri: impl Into<String>, port: u16) -> Result<Self, std::io::Error> {
        let std_listener = std::net::TcpListener::bind(format!("127.0.0.1:{port}"))?;
        std_listener.set_nonblocking(true)?;
        let listener = TcpListener::from_std(std_listener)?;
        Ok(Self { listener, redirect_uri: redirect_uri.into() })
    }
}

impl OAuthHandler for BrowserOAuthHandler {
    fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }

    fn authorize(&self, auth_url: &str) -> BoxFuture<'_, Result<String, OAuthError>> {
        let auth_url = auth_url.to_string();
        Box::pin(async move {
            if let Err(error) = open_browser(&auth_url) {
                tracing::warn!("Failed to open browser: {error}");
            }
            accept_oauth_callback(&self.listener).await
        })
    }
}

/// Accept an OAuth callback and return its absolute URL.
pub async fn accept_oauth_callback(listener: &TcpListener) -> Result<String, OAuthError> {
    loop {
        let (mut socket, _) = listener.accept().await?;
        let request_line = {
            let mut reader = BufReader::new(&mut socket);
            let mut line = String::new();
            let bytes_read =
                timeout(Duration::from_secs(2), reader.read_line(&mut line)).await.ok().and_then(Result::ok);
            let Some(1..) = bytes_read else { continue };
            line
        };

        match callback_url(&request_line) {
            Ok(callback_url) => {
                let _ = socket.write_all(success_response().as_bytes()).await;
                return Ok(callback_url);
            }
            Err(error) if request_line.contains('?') => return Err(error),
            Err(_) => {}
        }
    }
}

/// Start a local callback server and return the OAuth callback URL.
pub async fn wait_for_callback(port: u16) -> Result<String, OAuthError> {
    let listener = TcpListener::bind(format!("127.0.0.1:{port}")).await?;
    accept_oauth_callback(&listener).await
}

/// Open a URL in the default browser.
pub fn open_browser(url: &str) -> Result<(), OAuthError> {
    #[cfg(target_os = "macos")]
    {
        Command::new("open").arg(url).spawn().map_err(std::io::Error::other)?;
    }

    #[cfg(target_os = "linux")]
    {
        Command::new("xdg-open").arg(url).spawn().map_err(std::io::Error::other)?;
    }

    #[cfg(target_os = "windows")]
    {
        Command::new("cmd").args(["/C", "start", url]).spawn().map_err(std::io::Error::other)?;
    }

    Ok(())
}

fn callback_url(request_line: &str) -> Result<String, OAuthError> {
    let path = request_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| OAuthError::InvalidCallback("Invalid HTTP request format".to_string()))?;
    let url = url::Url::parse(&format!("http://localhost{path}"))
        .map_err(|error| OAuthError::InvalidCallback(format!("Invalid callback URL: {error}")))?;
    if url.query().is_none() {
        return Err(OAuthError::InvalidCallback("No query parameters in callback".to_string()));
    }
    if url.query_pairs().any(|(key, _)| key == "error") {
        return Err(OAuthError::InvalidCallback("OAuth authorization failed".to_string()));
    }
    Ok(url.to_string())
}

fn success_response() -> String {
    let body = include_str!("oauth_success.html");
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_request_preserves_encoded_parameters() {
        let callback = callback_url(
            "GET /oauth2callback?code=caf%C3%A9&state=test+state&iss=https%3A%2F%2Fauth.example.com HTTP/1.1\r\n",
        )
        .unwrap();
        let url = url::Url::parse(&callback).unwrap();
        let params = url.query_pairs().collect::<std::collections::HashMap<_, _>>();
        assert_eq!(params.get("code").map(AsRef::as_ref), Some("café"));
        assert_eq!(params.get("state").map(AsRef::as_ref), Some("test state"));
        assert_eq!(params.get("iss").map(AsRef::as_ref), Some("https://auth.example.com"));
    }

    #[test]
    fn callback_error_is_sanitized() {
        let error =
            callback_url("GET /oauth2callback?error=access_denied&error_description=attacker+controlled HTTP/1.1\r\n")
                .unwrap_err()
                .to_string();
        assert!(error.contains("OAuth authorization failed"));
        assert!(!error.contains("attacker"));
    }

    #[tokio::test]
    async fn callback_listener_skips_stale_requests() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = tokio::spawn(async move { accept_oauth_callback(&listener).await });

        let mut stale = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        stale.write_all(b"GET /favicon.ico HTTP/1.1\r\n").await.unwrap();
        let mut callback = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        callback.write_all(b"GET /?code=abc&state=xyz HTTP/1.1\r\n").await.unwrap();

        assert!(handle.await.unwrap().unwrap().contains("code=abc&state=xyz"));
    }

    #[tokio::test]
    async fn custom_redirect_uri_is_retained() {
        let handler = BrowserOAuthHandler::with_redirect_uri("http://localhost:9999/callback", 0).unwrap();
        assert_eq!(handler.redirect_uri(), "http://localhost:9999/callback");
    }
}
