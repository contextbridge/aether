use aether_core::mcp::{McpSpawnResult, mcp};
use aether_core::testing::{FakeMcpServer, fake_mcp};
use futures::future::BoxFuture;
use mcp_utils::client::oauth::{OAuthCallback, OAuthError, OAuthHandler, accept_oauth_callback};
use mcp_utils::client::{McpClientEvent, McpManager, McpServerConfig, ServerConfig};
use mcp_utils::status::{McpServerAuthCapability, McpServerStatus};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use std::sync::Arc;
use tokio::sync::mpsc;

struct FakeOAuthHandler {
    callback: OAuthCallback,
    redirect_uri: String,
}

impl FakeOAuthHandler {
    fn new(code: &str, state: &str) -> Self {
        Self {
            callback: OAuthCallback { code: code.to_string(), state: state.to_string() },
            redirect_uri: "http://127.0.0.1:0/oauth2callback".to_string(),
        }
    }
}

impl OAuthHandler for FakeOAuthHandler {
    fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }

    fn authorize(&self, _auth_url: &str) -> BoxFuture<'_, Result<OAuthCallback, OAuthError>> {
        let callback = self.callback.clone();
        Box::pin(async move { Ok(callback) })
    }
}

struct CancellingOAuthHandler;

impl OAuthHandler for CancellingOAuthHandler {
    fn redirect_uri(&self) -> &'static str {
        "http://127.0.0.1:0/oauth2callback"
    }

    fn authorize(&self, _auth_url: &str) -> BoxFuture<'_, Result<OAuthCallback, OAuthError>> {
        Box::pin(async { Err(OAuthError::UserCancelled) })
    }
}

#[tokio::test]
async fn fake_oauth_handler_returns_configured_callback() {
    let handler = FakeOAuthHandler::new("test_code", "test_state");
    let result = handler.authorize("https://example.com/auth").await;
    let callback = result.unwrap();
    assert_eq!(callback.code, "test_code");
    assert_eq!(callback.state, "test_state");
}

#[tokio::test]
async fn cancelling_handler_returns_user_cancelled() {
    let handler = CancellingOAuthHandler;
    let result = handler.authorize("https://example.com/auth").await;
    assert!(matches!(result, Err(OAuthError::UserCancelled)));
}

#[tokio::test]
async fn builder_with_oauth_handler_spawns_successfully() {
    let handler = FakeOAuthHandler::new("code", "state");

    let McpSpawnResult { tool_definitions, instructions, event_rx: _, .. } =
        mcp().with_oauth_handler(handler).with_servers(vec![]).spawn().await.unwrap();

    assert!(tool_definitions.is_empty());
    assert!(instructions.is_empty());
}

#[tokio::test]
async fn http_server_without_handler_returns_error() {
    let (event_tx, _event_rx) = mpsc::channel::<McpClientEvent>(50);
    let mut manager = mcp_utils::client::McpManager::new(event_tx, None);

    let config = StreamableHttpClientTransportConfig::with_uri("http://localhost:19999/mcp");
    let result = manager.add_mcp(ServerConfig::Http { name: "test_server".to_string(), config }.into()).await;

    assert!(result.is_err());
}

#[tokio::test]
async fn http_server_with_handler_stashes_needs_oauth_on_failure() {
    let handler = FakeOAuthHandler::new("test_code", "test_state");
    let (event_tx, _event_rx) = mpsc::channel::<McpClientEvent>(50);
    let mut manager = mcp_utils::client::McpManager::new(event_tx, Some(Arc::new(handler)));

    let config = StreamableHttpClientTransportConfig::with_uri("http://localhost:19999/mcp");
    let result = manager.add_mcp(ServerConfig::Http { name: "test_oauth_server".to_string(), config }.into()).await;

    // Connection fails, server should be stashed as NeedsOAuth (not auto-trigger OAuth)
    assert!(result.is_err());

    let statuses = manager.server_statuses();
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].name, "test_oauth_server");
    assert!(
        matches!(statuses[0].status, mcp_utils::status::McpServerStatus::NeedsOAuth),
        "Expected NeedsOAuth, got: {:?}",
        statuses[0].status
    );
    assert!(statuses[0].can_authenticate());
}

#[tokio::test]
async fn add_mcps_continues_on_oauth_failure() {
    let handler = FakeOAuthHandler::new("code", "state");
    let (event_tx, _event_rx) = mpsc::channel::<McpClientEvent>(50);
    let mut manager = mcp_utils::client::McpManager::new(event_tx, Some(Arc::new(handler)));

    let configs = vec![
        ServerConfig::Http {
            name: "failing_server_1".to_string(),
            config: StreamableHttpClientTransportConfig::with_uri("http://localhost:19998/mcp"),
        }
        .into(),
        ServerConfig::Http {
            name: "failing_server_2".to_string(),
            config: StreamableHttpClientTransportConfig::with_uri("http://localhost:19997/mcp"),
        }
        .into(),
    ];

    let result = manager.add_mcps(configs).await;
    assert!(result.is_ok());
    assert!(manager.tool_definitions().is_empty());
}

#[tokio::test]
async fn accept_oauth_callback_parses_code_and_state() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let callback_url = format!("http://127.0.0.1:{port}/oauth2callback?code=abc123&state=csrf_token");

    let handle = tokio::spawn(async move { accept_oauth_callback(&listener).await });

    // Give the callback server time to start
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let client = reqwest::Client::new();
    let _response = client.get(&callback_url).send().await.expect("Failed to send callback request");

    let result = handle.await.unwrap();
    let callback = result.unwrap();
    assert_eq!(callback.code, "abc123");
    assert_eq!(callback.state, "csrf_token");
}

#[tokio::test]
async fn oauth_handler_is_dyn_compatible() {
    let handler: Arc<dyn OAuthHandler> = Arc::new(FakeOAuthHandler::new("code", "state"));
    let result = handler.authorize("https://example.com").await;
    assert!(result.is_ok());
    assert_eq!(result.unwrap().code, "code");
}

#[tokio::test]
async fn tool_proxy_with_failing_http_surfaces_needs_oauth() {
    let handler = FakeOAuthHandler::new("code", "state");
    let (event_tx, _event_rx) = mpsc::channel::<McpClientEvent>(50);
    let mut manager = McpManager::new(event_tx, Some(Arc::new(handler)));

    // A tool-proxy with one in-memory server (succeeds) and one HTTP server (fails → NeedsOAuth)
    let configs = vec![McpServerConfig::ToolProxy {
        name: "proxy-oauth".to_string(),
        servers: vec![
            fake_mcp("local", FakeMcpServer::new()),
            ServerConfig::Http {
                name: "remote".to_string(),
                config: StreamableHttpClientTransportConfig::with_uri("http://localhost:19999/mcp"),
            },
        ],
    }];

    let _ = manager.add_mcps(configs).await;
    let statuses = manager.server_statuses();

    // The failing HTTP server should be stashed as NeedsOAuth
    let remote_status = statuses.iter().find(|s| s.name == "remote").expect("Expected status entry for 'remote'");
    assert!(
        matches!(remote_status.status, McpServerStatus::NeedsOAuth),
        "Expected NeedsOAuth for failing HTTP server, got: {:?}",
        remote_status.status
    );
    assert_eq!(remote_status.auth_capability, McpServerAuthCapability::OAuth);
    assert!(remote_status.can_authenticate());

    // The proxy itself should still be connected
    let proxy_status = statuses.iter().find(|s| s.name == "proxy-oauth").expect("Expected status entry for proxy");

    assert!(
        matches!(proxy_status.status, McpServerStatus::Connected { .. }),
        "Expected proxy to be Connected, got: {:?}",
        proxy_status.status
    );

    assert_eq!(proxy_status.auth_capability, McpServerAuthCapability::Unavailable);

    // The proxy's call_tool should still be available
    let defs = manager.tool_definitions();
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].name, "proxy-oauth__call_tool");
}

#[tokio::test]
async fn tool_proxy_partial_connection_works() {
    let (event_tx, _event_rx) = mpsc::channel::<McpClientEvent>(50);
    let mut manager = McpManager::new(event_tx, None);

    // A tool-proxy with two servers: one in-memory (succeeds), one HTTP (fails)
    let configs = vec![McpServerConfig::ToolProxy {
        name: "partial".to_string(),
        servers: vec![
            fake_mcp("working", FakeMcpServer::new()),
            ServerConfig::Http {
                name: "broken".to_string(),
                config: StreamableHttpClientTransportConfig::with_uri("http://localhost:19999/mcp"),
            },
        ],
    }];

    let _ = manager.add_mcps(configs).await;

    // The proxy should be connected with 1 tool (call_tool)
    let defs = manager.tool_definitions();
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].name, "partial__call_tool");

    // Instructions should mention the working server
    let instructions = manager.server_instructions();
    let proxy_instr = instructions.iter().find(|i| i.server_name == "partial").expect("Expected proxy instructions");
    assert!(proxy_instr.instructions.contains("working"), "Instructions should mention the connected server");
}
