#![cfg(feature = "mcp")]

use aether_auth::{
    FakeOAuthCredentialStore, OAuthClientRegistration, OAuthCredentialStorage, OAuthError, OAuthFlowOptions,
    OAuthHandler, create_auth_manager_from_store, perform_oauth_flow,
};
use futures::future::BoxFuture;
use rmcp::transport::auth::StoredCredentials;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn configured_client_id_ignores_credentials_for_another_client() {
    let stored = StoredCredentials::new("old-client".to_string(), None, Vec::new(), None)
        .with_issuer(Some("https://old.example".to_string()));
    let store =
        Arc::new(FakeOAuthCredentialStore::new().with_value("mcp:slack", serde_json::to_value(stored).unwrap()));

    let restored = create_auth_manager_from_store(
        "slack",
        "https://mcp.slack.com/mcp",
        Some("configured-client"),
        "http://localhost:3118/",
        store.clone(),
    )
    .await
    .unwrap();

    assert!(restored.is_none());
    assert_eq!(store.load("mcp:slack").await.unwrap().unwrap()["client_id"], "old-client");
}

struct FakeHandler {
    auth_url: Arc<Mutex<Option<String>>>,
    issuer: Option<String>,
}

impl OAuthHandler for FakeHandler {
    fn redirect_uri(&self) -> &'static str {
        "http://localhost:3118/"
    }

    fn authorize(&self, auth_url: &str) -> BoxFuture<'_, Result<String, OAuthError>> {
        *self.auth_url.lock().unwrap() = Some(auth_url.to_string());
        let state =
            url::Url::parse(auth_url).unwrap().query_pairs().find(|(name, _)| name == "state").unwrap().1.into_owned();
        let issuer = self.issuer.clone();
        Box::pin(async move {
            let mut callback = url::Url::parse("http://localhost:3118/").unwrap();
            callback.query_pairs_mut().append_pair("code", "test-code").append_pair("state", &state);
            if let Some(issuer) = issuer {
                callback.query_pairs_mut().append_pair("iss", &issuer);
            }
            Ok(callback.to_string())
        })
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct OAuthServerOptions {
    supports_cimd: bool,
    requires_response_issuer: bool,
}

struct OAuthServer {
    base_url: String,
    requests: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}

impl OAuthServer {
    async fn bind(options: OAuthServerOptions) -> Self {
        let OAuthServerOptions { supports_cimd, requires_response_issuer } = options;
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
                captured_requests.lock().unwrap().push(request.to_string());
                let path = request.lines().next().unwrap().split_whitespace().nth(1).unwrap();
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
                        serde_json::json!({"access_token": "access-token", "token_type": "Bearer", "expires_in": 3600})
                    } else if path == "/register" {
                        serde_json::json!({
                            "client_id": "registered-client",
                            "redirect_uris": ["http://localhost:3118/"]
                        })
                    } else {
                        let mut metadata = serde_json::json!({
                            "issuer": origin,
                            "authorization_endpoint": format!("{origin}/authorize"),
                            "token_endpoint": format!("{origin}/token"),
                            "registration_endpoint": format!("{origin}/register"),
                            "response_types_supported": ["code"],
                            "code_challenge_methods_supported": ["S256"],
                            "scopes_supported": ["openid"]
                        });
                        if supports_cimd {
                            metadata["client_id_metadata_document_supported"] = serde_json::Value::Bool(true);
                        }
                        if requires_response_issuer {
                            metadata["authorization_response_iss_parameter_supported"] = serde_json::Value::Bool(true);
                        }
                        metadata
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
async fn configured_cimd_is_forwarded_to_rmcp() {
    let server = OAuthServer::bind(OAuthServerOptions { supports_cimd: true, ..OAuthServerOptions::default() }).await;
    let auth_url = Arc::new(Mutex::new(None));
    let handler = FakeHandler { auth_url: Arc::clone(&auth_url), issuer: None };
    let metadata_url = "https://aether-agent.io/oauth/client-metadata.json";

    perform_oauth_flow(
        "slack",
        &server.base_url,
        &handler,
        OAuthFlowOptions {
            client_registration: OAuthClientRegistration::ClientMetadata(metadata_url.to_string()),
            ..OAuthFlowOptions::default()
        },
        None,
    )
    .await
    .unwrap();

    let auth_url = url::Url::parse(auth_url.lock().unwrap().as_ref().unwrap()).unwrap();
    assert!(auth_url.query_pairs().any(|(name, value)| name == "client_id" && value == metadata_url));
    assert!(!server.requests.lock().unwrap().iter().any(|request| request.starts_with("POST /register")));
}

#[tokio::test]
async fn dynamic_registration_is_explicitly_selected() {
    let server = OAuthServer::bind(OAuthServerOptions::default()).await;
    let auth_url = Arc::new(Mutex::new(None));
    let handler = FakeHandler { auth_url: Arc::clone(&auth_url), issuer: None };

    perform_oauth_flow("slack", &server.base_url, &handler, OAuthFlowOptions::default(), None).await.unwrap();

    let auth_url = url::Url::parse(auth_url.lock().unwrap().as_ref().unwrap()).unwrap();
    assert!(auth_url.query_pairs().any(|(name, value)| name == "client_id" && value == "registered-client"));
    assert!(server.requests.lock().unwrap().iter().any(|request| request.starts_with("POST /register")));
}

#[tokio::test]
async fn configured_client_id_is_forwarded_to_rmcp() {
    let server = OAuthServer::bind(OAuthServerOptions::default()).await;
    let auth_url = Arc::new(Mutex::new(None));
    let handler = FakeHandler { auth_url: Arc::clone(&auth_url), issuer: None };

    perform_oauth_flow(
        "slack",
        &server.base_url,
        &handler,
        OAuthFlowOptions {
            client_registration: OAuthClientRegistration::PreRegistered("static-client".to_string()),
            ..OAuthFlowOptions::default()
        },
        None,
    )
    .await
    .unwrap();

    let auth_url = url::Url::parse(auth_url.lock().unwrap().as_ref().unwrap()).unwrap();
    assert!(auth_url.query_pairs().any(|(name, value)| name == "client_id" && value == "static-client"));
    assert!(!server.requests.lock().unwrap().iter().any(|request| request.starts_with("POST /register")));
}

#[tokio::test]
async fn callback_url_is_delegated_to_rmcp_for_issuer_validation() {
    let server =
        OAuthServer::bind(OAuthServerOptions { requires_response_issuer: true, ..OAuthServerOptions::default() }).await;
    let issuer = server.base_url.trim_end_matches("/mcp").to_string();
    let handler = FakeHandler { auth_url: Arc::new(Mutex::new(None)), issuer: Some(issuer) };

    perform_oauth_flow(
        "slack",
        &server.base_url,
        &handler,
        OAuthFlowOptions {
            client_registration: OAuthClientRegistration::PreRegistered("static-client".to_string()),
            ..OAuthFlowOptions::default()
        },
        None,
    )
    .await
    .unwrap();

    assert!(server.requests.lock().unwrap().iter().any(|request| request.starts_with("POST /token")));
}
