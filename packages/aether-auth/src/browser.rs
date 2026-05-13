use crate::error::OAuthError;
use crate::handler::{OAuthCallback, OAuthHandler};
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
    ///
    /// Use this when the OAuth provider has a fixed redirect URI registered
    /// (e.g. `http://localhost:1455/auth/callback` for Codex).
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

    fn authorize(&self, auth_url: &str) -> BoxFuture<'_, Result<OAuthCallback, OAuthError>> {
        let auth_url = auth_url.to_string();
        Box::pin(async move {
            if let Err(e) = open_browser(&auth_url) {
                tracing::warn!("Failed to open browser: {e}. Open manually: {auth_url}");
            }

            accept_oauth_callback(&self.listener).await
        })
    }
}

/// Accept an OAuth callback on an already-bound listener.
///
/// Loops until it receives a valid OAuth callback, skipping stale connections
/// left over from previous flows (e.g. browser favicon requests, preconnect
/// sockets, or closed connections).
pub async fn accept_oauth_callback(listener: &TcpListener) -> Result<OAuthCallback, OAuthError> {
    loop {
        let (mut socket, _) = listener.accept().await?;
        let request_line = {
            let mut reader = BufReader::new(&mut socket);
            let mut line = String::new();
            match timeout(Duration::from_secs(2), reader.read_line(&mut line)).await {
                Ok(Ok(1..)) => line,
                _ => continue,
            }
        };

        match parse_callback_from_request(&request_line) {
            Ok(callback) => {
                let _ = socket.write_all(create_success_response().as_bytes()).await;
                return Ok(callback);
            }
            Err(e) => {
                if request_line.contains('?') {
                    return Err(e);
                }
            }
        }
    }
}

/// Start a local callback server to capture the OAuth authorization code and state
///
/// Listens on the specified port and waits for the OAuth redirect.
/// Returns the authorization code and state (CSRF token) from the callback URL.
pub async fn wait_for_callback(port: u16) -> Result<OAuthCallback, OAuthError> {
    let addr = format!("127.0.0.1:{port}");
    let listener = TcpListener::bind(&addr).await?;
    accept_oauth_callback(&listener).await
}

/// Open a URL in the default browser
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

/// Parse the authorization code and state from the HTTP request line
fn parse_callback_from_request(request_line: &str) -> Result<OAuthCallback, OAuthError> {
    // Request format: GET /oauth2callback?code=XXX&state=YYY HTTP/1.1
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 2 {
        return Err(OAuthError::InvalidCallback("Invalid HTTP request format".to_string()));
    }

    let path = parts[1];
    let query_start =
        path.find('?').ok_or_else(|| OAuthError::InvalidCallback("No query parameters in callback".to_string()))?;

    let query = &path[query_start + 1..];

    // Check for error in callback
    for param in query.split('&') {
        if let Some((key, value)) = param.split_once('=')
            && key == "error"
        {
            let error_desc = query
                .split('&')
                .find_map(|p| {
                    p.split_once('=').filter(|(k, _)| *k == "error_description").map(|(_, v)| urlencoding_decode(v))
                })
                .unwrap_or_else(|| value.to_string());
            return Err(OAuthError::InvalidCallback(format!("OAuth error: {error_desc}")));
        }
    }

    // Extract code and state
    let mut code = None;
    let mut state = None;

    for param in query.split('&') {
        if let Some((key, value)) = param.split_once('=') {
            match key {
                "code" => code = Some(urlencoding_decode(value)),
                "state" => state = Some(urlencoding_decode(value)),
                _ => {}
            }
        }
    }

    let code = code.ok_or_else(|| OAuthError::InvalidCallback("No authorization code in callback".into()))?;
    let state = state.ok_or_else(|| OAuthError::InvalidCallback("No state parameter in callback".into()))?;

    Ok(OAuthCallback { code, state })
}

/// Simple URL decoding (handles %XX escapes)
fn urlencoding_decode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                result.push(byte as char);
            } else {
                result.push('%');
                result.push_str(&hex);
            }
        } else if c == '+' {
            result.push(' ');
        } else {
            result.push(c);
        }
    }

    result
}

/// Create an HTML success response
fn create_success_response() -> String {
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
    fn parse_callback_from_valid_request() {
        let request = "GET /oauth2callback?code=4%2F0AYWS-abc123&state=verifier HTTP/1.1\r\n";
        let callback = parse_callback_from_request(request).unwrap();
        assert_eq!(callback.code, "4/0AYWS-abc123");
        assert_eq!(callback.state, "verifier");
    }

    #[test]
    fn parse_callback_handles_plus_encoding() {
        let request = "GET /oauth2callback?code=hello+world&state=test+state HTTP/1.1\r\n";
        let callback = parse_callback_from_request(request).unwrap();
        assert_eq!(callback.code, "hello world");
        assert_eq!(callback.state, "test state");
    }

    #[test]
    fn parse_callback_returns_error_for_oauth_error() {
        let request = "GET /oauth2callback?error=access_denied&error_description=User+denied HTTP/1.1\r\n";
        let result = parse_callback_from_request(request);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("User denied"));
    }

    #[test]
    fn parse_callback_returns_error_for_missing_code() {
        let request = "GET /oauth2callback?state=verifier HTTP/1.1\r\n";
        let result = parse_callback_from_request(request);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No authorization code"));
    }

    #[test]
    fn parse_callback_returns_error_for_missing_state() {
        let request = "GET /oauth2callback?code=abc123 HTTP/1.1\r\n";
        let result = parse_callback_from_request(request);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No state parameter"));
    }

    #[tokio::test]
    async fn with_redirect_uri_binds_to_specified_port() {
        let handler = BrowserOAuthHandler::with_redirect_uri("http://localhost:9999/callback", 0).unwrap();
        assert_eq!(handler.redirect_uri(), "http://localhost:9999/callback");
    }

    #[test]
    fn urlencoding_decode_handles_percent() {
        assert_eq!(urlencoding_decode("hello%20world"), "hello world");
        assert_eq!(urlencoding_decode("a%2Fb"), "a/b");
    }

    #[test]
    fn urlencoding_decode_handles_plus() {
        assert_eq!(urlencoding_decode("hello+world"), "hello world");
    }

    #[tokio::test]
    async fn accept_oauth_callback_skips_stale_favicon_request() {
        use tokio::io::AsyncWriteExt;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = tokio::spawn(async move { accept_oauth_callback(&listener).await });

        let mut stale = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
        stale.write_all(b"GET /favicon.ico HTTP/1.1\r\n").await.unwrap();

        let mut real = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
        real.write_all(b"GET /oauth2callback?code=abc&state=xyz HTTP/1.1\r\n").await.unwrap();

        let callback = handle.await.unwrap().unwrap();
        assert_eq!(callback.code, "abc");
        assert_eq!(callback.state, "xyz");
    }

    #[tokio::test]
    async fn accept_oauth_callback_skips_closed_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = tokio::spawn(async move { accept_oauth_callback(&listener).await });

        {
            let _stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
        }

        let mut real = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
        real.write_all(b"GET /oauth2callback?code=def&state=uvw HTTP/1.1\r\n").await.unwrap();

        let callback = handle.await.unwrap().unwrap();
        assert_eq!(callback.code, "def");
        assert_eq!(callback.state, "uvw");
    }
}
