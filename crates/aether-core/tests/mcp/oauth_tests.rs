use aether_auth::{OAuthError, OAuthHandler, accept_oauth_callback};
use aether_core::mcp::mcp;
use aether_core::testing::{FakeMcpServer, fake_mcp};
use futures::future::BoxFuture;
use mcp_utils::client::{
    DeferredToolRules, ElicitingOAuthHandler, McpClientEvent, McpManager, OAuthHandlerContext, OAuthHandlerFactory,
    RuntimeMcpServer, RuntimeMcpTransport, ToolExposure,
};
use mcp_utils::status::{McpServerAuthCapability, McpServerStatus};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
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

    fn server(&self, name: &str, exposure: ToolExposure) -> RuntimeMcpServer {
        RuntimeMcpServer::new(
            name,
            RuntimeMcpTransport::Http(StreamableHttpClientTransportConfig::with_uri(self.uri.as_str()).into()),
            exposure,
        )
    }
}

impl Drop for FailingHttpEndpoint {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct UnauthorizedHttpEndpoint {
    uri: String,
    task: JoinHandle<()>,
}

impl UnauthorizedHttpEndpoint {
    async fn bind(challenge: &'static str) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let uri = format!("http://{}/mcp", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut request = [0; 4096];
                let _ = stream.read(&mut request).await;
                let response = format!(
                    "HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: {challenge}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
                let _ = stream.write_all(response.as_bytes()).await;
            }
        });
        Self { uri, task }
    }

    fn server(&self, name: &str) -> RuntimeMcpServer {
        RuntimeMcpServer::new(
            name,
            RuntimeMcpTransport::Http(StreamableHttpClientTransportConfig::with_uri(self.uri.as_str()).into()),
            ToolExposure::ModelVisible,
        )
    }
}

impl Drop for UnauthorizedHttpEndpoint {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn test_manager(with_oauth: bool) -> McpManager {
    let (event_tx, _) = mpsc::channel::<McpClientEvent>(50);
    let factory = if with_oauth { Some(fake_oauth_handler_factory()) } else { None };
    McpManager::new(event_tx, factory)
}

struct CancellingOAuthHandler;

impl OAuthHandler for CancellingOAuthHandler {
    fn redirect_uri(&self) -> &'static str {
        "http://127.0.0.1:0/oauth2callback"
    }

    fn authorize(&self, _auth_url: &str) -> BoxFuture<'_, Result<String, OAuthError>> {
        Box::pin(async { Err(OAuthError::UserCancelled) })
    }
}

fn fake_oauth_handler_factory() -> OAuthHandlerFactory {
    Arc::new(|_ctx| Ok(Arc::new(CancellingOAuthHandler)))
}

#[tokio::test]
async fn cancelling_handler_returns_user_cancelled() {
    let handler = CancellingOAuthHandler;
    let result = handler.authorize("https://example.com/auth").await;
    assert!(matches!(result, Err(OAuthError::UserCancelled)));
}

#[tokio::test]
async fn eliciting_oauth_handler_emits_completion_after_callback() {
    let (event_tx, mut event_rx) = mpsc::channel(4);
    let handler = Arc::new(
        ElicitingOAuthHandler::new(OAuthHandlerContext {
            server_name: "linear".to_string(),
            callback_port: None,
            tx: event_tx,
        })
        .unwrap(),
    );
    let callback_url = handler.redirect_uri().to_string();
    let authorize = {
        let handler = handler.clone();
        tokio::spawn(async move { handler.authorize("https://linear.app/oauth").await })
    };

    let McpClientEvent::Elicitation(request) = event_rx.recv().await.unwrap() else {
        panic!("expected OAuth elicitation");
    };
    request.response_sender.send(rmcp::model::ElicitResult::new(rmcp::model::ElicitationAction::Accept)).unwrap();
    let port = callback_url
        .strip_prefix("http://localhost:")
        .and_then(|value| value.strip_suffix('/'))
        .unwrap()
        .parse::<u16>()
        .unwrap();
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    stream.write_all(b"GET /?code=test-code&state=test-state HTTP/1.1\r\nHost: localhost\r\n\r\n").await.unwrap();
    authorize.await.unwrap().unwrap();

    let completion = event_rx.try_recv().expect("OAuth callback should complete the URL elicitation");
    assert!(matches!(
        completion,
        McpClientEvent::ElicitationComplete { server_name, elicitation_id }
            if server_name == "linear" && elicitation_id == "aether-oauth"
    ));
}

#[tokio::test]
async fn builder_with_oauth_handler_factory_spawns_successfully() {
    let handler = Arc::new(CancellingOAuthHandler);

    let mut spawn = mcp("/workspace")
        .with_oauth_handler_factory(Arc::new(move |_ctx| Ok(handler.clone())))
        .with_servers(vec![])
        .spawn()
        .await
        .unwrap();
    let snapshot = spawn.block_until_ready().await.expect("bootstrap completes");

    assert!(snapshot.tool_definitions().is_empty());
    assert!(snapshot.model_instructions().is_empty());
}

#[tokio::test]
async fn http_server_without_handler_stashes_failed_status() {
    let endpoint = FailingHttpEndpoint::bind().await;
    let mut manager = test_manager(false);
    assert!(manager.add_mcps(vec![endpoint.server("test_server", ToolExposure::ModelVisible)]).await.is_ok());
}

#[tokio::test]
async fn real_401_challenge_is_classified_as_needs_oauth() {
    let endpoint = UnauthorizedHttpEndpoint::bind(
        r#"Bearer resource_metadata="https://example.com/.well-known/oauth-protected-resource", scope="tools:read""#,
    )
    .await;
    let mut manager = test_manager(true);

    manager.add_mcps(vec![endpoint.server("protected")]).await.unwrap();

    let status = &manager.server_statuses()[0];
    assert!(matches!(status.status, McpServerStatus::NeedsOAuth));
    assert_eq!(status.auth_capability, McpServerAuthCapability::OAuth);
    assert!(status.can_authenticate());
}

#[tokio::test]
async fn http_server_with_handler_classifies_non_auth_failure_as_failed() {
    let endpoint = FailingHttpEndpoint::bind().await;
    let mut manager = test_manager(true);

    assert!(manager.add_mcps(vec![endpoint.server("test_oauth_server", ToolExposure::ModelVisible)]).await.is_ok());

    let statuses = manager.server_statuses();
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].name, "test_oauth_server");
    assert!(
        matches!(statuses[0].status, McpServerStatus::Failed { .. }),
        "Expected Failed, got: {:?}",
        statuses[0].status
    );
    assert!(!statuses[0].can_authenticate());
}

#[tokio::test]
async fn add_mcps_continues_on_oauth_failure() {
    let endpoint = FailingHttpEndpoint::bind().await;
    let mut manager = test_manager(true);

    assert!(
        manager
            .add_mcps(vec![
                endpoint.server("failing_server_1", ToolExposure::ModelVisible),
                endpoint.server("failing_server_2", ToolExposure::ModelVisible)
            ])
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

    let client = reqwest::Client::new();
    let _response = client.get(&callback_url).send().await.expect("Failed to send callback request");

    let result = handle.await.unwrap();
    let callback_url = result.unwrap();
    let callback = reqwest::Url::parse(&callback_url).unwrap();
    let params = callback.query_pairs().collect::<std::collections::HashMap<_, _>>();
    assert_eq!(params.get("code").map(AsRef::as_ref), Some("abc123"));
    assert_eq!(params.get("state").map(AsRef::as_ref), Some("csrf_token"));
}

#[tokio::test]
async fn deferred_server_with_failing_http_surfaces_failure() {
    let endpoint = FailingHttpEndpoint::bind().await;
    let mut manager = test_manager(true);

    let servers = vec![
        fake_mcp("local", FakeMcpServer::new()).with_exposure(ToolExposure::deferred_all()),
        endpoint.server("remote", ToolExposure::deferred_all()),
    ];

    let _ = manager.add_mcps(servers).await;
    let statuses = manager.server_statuses();

    let remote_status = statuses.iter().find(|s| s.name == "remote").expect("Expected status entry for 'remote'");
    assert!(
        matches!(remote_status.status, McpServerStatus::Failed { .. }),
        "Expected Failed for failing HTTP server, got: {:?}",
        remote_status.status
    );
    assert_eq!(remote_status.auth_capability, McpServerAuthCapability::Unavailable);
    assert!(!remote_status.can_authenticate());
    assert!(remote_status.deferred_tools, "Expected remote to have deferred tools");

    let local_status = statuses.iter().find(|s| s.name == "local").expect("Expected status entry for 'local'");
    assert!(matches!(local_status.status, McpServerStatus::Connected { .. }));
    assert!(local_status.deferred_tools);
    assert!(manager.tool_definitions().is_empty());
}

#[tokio::test]
async fn selective_policy_survives_reconnection_after_failure() {
    let endpoint = FailingHttpEndpoint::bind().await;
    let mut manager = test_manager(true);
    let selective = endpoint.server("remote", ToolExposure::Deferred(DeferredToolRules::new(&[], &["add_*"])));
    manager.add_mcps(vec![selective]).await.unwrap();

    assert!(manager.authenticate_server_task("remote").await.is_err());

    let connected = fake_mcp("remote", FakeMcpServer::new()).with_exposure(ToolExposure::deferred_all());
    let attempt = manager.connect_pending_task(connected).await;
    manager.apply_connection_attempt(attempt).await;

    let remote = manager.server_statuses().into_iter().find(|status| status.name == "remote").unwrap();
    assert!(matches!(remote.status, McpServerStatus::Connected { .. }));
    assert!(remote.deferred_tools);
    let names = manager.tool_definitions().into_iter().map(|tool| tool.name).collect::<Vec<_>>();
    assert_eq!(names, ["remote__add_numbers"]);
}

#[tokio::test]
async fn deferred_tools_partial_connection_works() {
    let endpoint = FailingHttpEndpoint::bind().await;
    let mut manager = test_manager(false);

    let servers = vec![
        fake_mcp("working", FakeMcpServer::new()).with_exposure(ToolExposure::deferred_all()),
        endpoint.server("broken", ToolExposure::deferred_all()),
    ];

    let _ = manager.add_mcps(servers).await;

    assert!(manager.tool_definitions().is_empty());
    let statuses = manager.server_statuses();
    assert!(matches!(
        statuses.iter().find(|status| status.name == "working").unwrap().status,
        McpServerStatus::Connected { .. }
    ));
    assert!(matches!(
        statuses.iter().find(|status| status.name == "broken").unwrap().status,
        McpServerStatus::Failed { .. }
    ));
}
