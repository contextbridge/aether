#![cfg(feature = "mcp")]

use aether_auth::{
    FakeOAuthCredentialStore, OAuthCallback, OAuthCredential, OAuthCredentialStorage, OAuthError, OAuthHandler,
    create_auth_manager_from_store, perform_oauth_flow,
};
use futures::future::BoxFuture;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn configured_client_id_ignores_credentials_for_another_client() {
    let store = Arc::new(FakeOAuthCredentialStore::new().with_credential(
        "slack",
        OAuthCredential {
            client_id: "old-client".to_string(),
            access_token: "old-token".to_string(),
            refresh_token: None,
            expires_at: None,
            granted_scopes: Vec::new(),
        },
    ));

    let manager =
        create_auth_manager_from_store("slack", "https://mcp.slack.com/mcp", Some("configured-client"), store.clone())
            .await
            .unwrap();

    assert!(manager.is_none());
    let preserved = store.load_credential("slack").await.unwrap().expect("mismatched credential should be preserved");
    assert_eq!(preserved.client_id, "old-client");
}

struct FakeHandler {
    auth_url: Arc<Mutex<Option<String>>>,
}

impl OAuthHandler for FakeHandler {
    fn redirect_uri(&self) -> &'static str {
        "http://localhost:3118/"
    }

    fn authorize(&self, auth_url: &str) -> BoxFuture<'_, Result<OAuthCallback, OAuthError>> {
        *self.auth_url.lock().unwrap() = Some(auth_url.to_string());
        let state =
            url::Url::parse(auth_url).unwrap().query_pairs().find(|(name, _)| name == "state").unwrap().1.into_owned();
        Box::pin(async move { Ok(OAuthCallback { code: "test-code".to_string(), state }) })
    }
}

struct OAuthServer {
    base_url: String,
    requests: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}

impl OAuthServer {
    async fn bind() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let base_url = format!("{origin}/mcp");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured_requests = Arc::clone(&requests);
        let task = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut buffer = vec![0; 8192];
                let read = stream.read(&mut buffer).await.unwrap();
                let request = String::from_utf8_lossy(&buffer[..read]);
                let request_line = request.lines().next().unwrap().to_string();
                captured_requests.lock().unwrap().push(request_line.clone());
                let path = request_line.split_whitespace().nth(1).unwrap();

                let (status, headers, body) = if path == "/mcp" {
                    (
                        "401 Unauthorized",
                        format!(
                            "WWW-Authenticate: Bearer resource_metadata=\"{origin}/.well-known/oauth-protected-resource/mcp\"\r\n"
                        ),
                        String::new(),
                    )
                } else {
                    let body = if path.contains("oauth-protected-resource") {
                        serde_json::json!({
                            "resource": format!("{origin}/mcp"),
                            "authorization_servers": [&origin]
                        })
                    } else if path == "/token" {
                        serde_json::json!({
                            "access_token": "access-token",
                            "token_type": "Bearer",
                            "expires_in": 3600
                        })
                    } else if path == "/register" {
                        serde_json::json!({
                            "client_id": "registered-client",
                            "redirect_uris": ["http://localhost:3118/"]
                        })
                    } else {
                        serde_json::json!({
                            "issuer": origin,
                            "authorization_endpoint": format!("{origin}/authorize"),
                            "token_endpoint": format!("{origin}/token"),
                            "registration_endpoint": format!("{origin}/register"),
                            "response_types_supported": ["code"],
                            "code_challenge_methods_supported": ["S256"],
                            "scopes_supported": ["openid"]
                        })
                    }
                    .to_string();
                    ("200 OK", String::new(), body)
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });
        Self { base_url, requests, task }
    }
}

impl Drop for OAuthServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[tokio::test]
async fn no_client_id_uses_dynamic_registration() {
    let server = OAuthServer::bind().await;
    let auth_url = Arc::new(Mutex::new(None));
    let handler = FakeHandler { auth_url: Arc::clone(&auth_url) };

    perform_oauth_flow("slack", &server.base_url, &handler, None, None).await.unwrap();

    let auth_url = auth_url.lock().unwrap().clone().unwrap();
    let auth_url = url::Url::parse(&auth_url).unwrap();
    assert!(auth_url.query_pairs().any(|(name, value)| name == "client_id" && value == "registered-client"));
    assert!(server.requests.lock().unwrap().iter().any(|request| request.starts_with("POST /register")));
}

#[tokio::test]
async fn configured_client_id_skips_dynamic_registration() {
    let server = OAuthServer::bind().await;
    let auth_url = Arc::new(Mutex::new(None));
    let handler = FakeHandler { auth_url: Arc::clone(&auth_url) };

    perform_oauth_flow("slack", &server.base_url, &handler, Some("static-client"), None).await.unwrap();

    let auth_url = auth_url.lock().unwrap().clone().unwrap();
    let auth_url = url::Url::parse(&auth_url).unwrap();
    assert!(auth_url.query_pairs().any(|(name, value)| name == "client_id" && value == "static-client"));
    assert!(auth_url.query_pairs().any(|(name, value)| name == "redirect_uri" && value == "http://localhost:3118/"));
    assert!(!server.requests.lock().unwrap().iter().any(|request| request.starts_with("POST /register")));
}
