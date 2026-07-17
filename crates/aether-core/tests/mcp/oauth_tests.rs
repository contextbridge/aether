use aether_auth::{OAuthCallback, OAuthError, OAuthHandler, accept_oauth_callback};
use aether_core::mcp::mcp;
use aether_core::testing::{FakeMcpServer, fake_mcp_with_proxy};
use futures::future::BoxFuture;
use mcp_utils::client::{McpClientEvent, McpManager, McpServer, McpTransport, OAuthHandlerFactory};
use mcp_utils::status::{McpServerAuthCapability, McpServerStatus};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

struct FailingHttpEndpoint {
    uri: String,
    task: JoinHandle<()>,
}

impl FailingHttpEndpoint {
    async fn bind() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let uri = format!("http://{}/mcp", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                drop(stream);
            }
        });
        Self { uri, task }
    }

    fn server(&self, name: &str, proxied: bool) -> McpServer {
        McpServer::new(
            name,
            McpTransport::Http(StreamableHttpClientTransportConfig::with_uri(self.uri.as_str()).into()),
            proxied,
        )
    }
}

impl Drop for FailingHttpEndpoint {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn test_manager(with_oauth: bool) -> McpManager {
    let (event_tx, _) = mpsc::channel::<McpClientEvent>(50);
    let factory = if with_oauth { Some(fake_oauth_handler_factory()) } else { None };
    McpManager::new(event_tx, factory)
}

fn test_manager_with_home(with_oauth: bool) -> (tempfile::TempDir, McpManager) {
    let home = tempfile::tempdir().unwrap();
    let (event_tx, _) = mpsc::channel::<McpClientEvent>(50);
    let factory = if with_oauth { Some(fake_oauth_handler_factory()) } else { None };
    let manager = McpManager::new(event_tx, factory).with_aether_home(home.path());
    (home, manager)
}

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

fn fake_oauth_handler_factory() -> OAuthHandlerFactory {
    Arc::new(|_ctx| Ok(Arc::new(CancellingOAuthHandler)))
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
async fn builder_with_oauth_handler_factory_spawns_successfully() {
    let handler = Arc::new(FakeOAuthHandler::new("code", "state"));

    let mut spawn = mcp("/workspace")
        .with_oauth_handler_factory(Arc::new(move |_ctx| Ok(handler.clone())))
        .with_servers(vec![])
        .spawn()
        .await
        .unwrap();
    let snapshot = spawn.block_until_ready().await.expect("bootstrap completes");

    assert!(snapshot.tool_definitions.is_empty());
    assert!(snapshot.instructions.is_empty());
}

#[tokio::test]
async fn http_server_without_handler_stashes_failed_status() {
    let endpoint = FailingHttpEndpoint::bind().await;
    let mut manager = test_manager(false);
    assert!(manager.add_mcps(vec![endpoint.server("test_server", false)]).await.is_ok());
}

#[tokio::test]
async fn http_server_with_handler_stashes_needs_oauth_on_failure() {
    let endpoint = FailingHttpEndpoint::bind().await;
    let mut manager = test_manager(true);

    assert!(manager.add_mcps(vec![endpoint.server("test_oauth_server", false)]).await.is_ok());

    let statuses = manager.server_statuses();
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].name, "test_oauth_server");
    assert!(
        matches!(statuses[0].status, McpServerStatus::NeedsOAuth),
        "Expected NeedsOAuth, got: {:?}",
        statuses[0].status
    );
    assert!(statuses[0].can_authenticate());
}

#[tokio::test]
async fn add_mcps_continues_on_oauth_failure() {
    let endpoint = FailingHttpEndpoint::bind().await;
    let mut manager = test_manager(true);

    assert!(
        manager
            .add_mcps(vec![endpoint.server("failing_server_1", false), endpoint.server("failing_server_2", false)])
            .await
            .is_ok()
    );
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
    let endpoint = FailingHttpEndpoint::bind().await;
    let (_home, mut manager) = test_manager_with_home(true);

    let servers = vec![fake_mcp_with_proxy("local", FakeMcpServer::new(), true), endpoint.server("remote", true)];

    let _ = manager.add_mcps(servers).await;
    let statuses = manager.server_statuses();

    let remote_status = statuses.iter().find(|s| s.name == "remote").expect("Expected status entry for 'remote'");
    assert!(
        matches!(remote_status.status, McpServerStatus::NeedsOAuth),
        "Expected NeedsOAuth for failing HTTP server, got: {:?}",
        remote_status.status
    );
    assert_eq!(remote_status.auth_capability, McpServerAuthCapability::OAuth);
    assert!(remote_status.can_authenticate());
    assert!(remote_status.proxied, "Expected remote to be marked as proxied");

    let local_status = statuses.iter().find(|s| s.name == "local").expect("Expected status entry for 'local'");
    assert!(matches!(local_status.status, McpServerStatus::Connected { .. }));
    assert!(local_status.proxied);
    assert!(!statuses.iter().any(|s| s.name == "proxy"));

    let defs = manager.tool_definitions();
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].name, "proxy__call_tool");
}

#[tokio::test]
async fn tool_proxy_partial_connection_works() {
    let endpoint = FailingHttpEndpoint::bind().await;
    let (_home, mut manager) = test_manager_with_home(false);

    let servers = vec![fake_mcp_with_proxy("working", FakeMcpServer::new(), true), endpoint.server("broken", true)];

    let _ = manager.add_mcps(servers).await;

    let defs = manager.tool_definitions();
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].name, "proxy__call_tool");

    let instructions = manager.server_instructions();
    let proxy_instr = instructions.get("proxy").expect("Expected proxy instructions");
    assert!(proxy_instr.contains("working"), "Instructions should mention the connected server");
}
