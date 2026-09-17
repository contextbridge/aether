use aether_tool_server::{HttpOptions, RemoteConfig, RemoteToolRuntime, serve};
use mcp_utils::{
    client::McpConfig,
    request_context::{AgentIdentity, GatewayRequestContext},
    tool_exposure::ToolExposure,
    tool_policy::ToolFilter,
};
use rmcp::{ClientLifecycleMode, model::*, serve_client_with_lifecycle, transport::StreamableHttpClientTransport};
use serde_json::json;
use tokio_util::sync::CancellationToken;
use utils::variables::Vars;

#[tokio::test]
async fn independent_http_requests_enforce_policy_and_agent_read_state() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("existing.txt"), "original").unwrap();
    let config =
        McpConfig::from_json(r#"{"servers":{"coding":{"type":"in-memory","args":["--disable-lsp"]}}}"#).unwrap();
    let mut runtime =
        RemoteToolRuntime::new(RemoteConfig::new(workspace.path(), config, &Vars::new()).unwrap()).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let cancellation = CancellationToken::new();
    let server = tokio::spawn(serve(listener, runtime.clone(), HttpOptions::default(), cancellation.clone()));
    let client = serve_client_with_lifecycle(
        (),
        StreamableHttpClientTransport::from_uri(format!("http://{address}/mcp")),
        ClientLifecycleMode::Discover { preferred_versions: vec![ProtocolVersion::V_2026_07_28] },
    )
    .await
    .unwrap();
    assert!(client.peer_info().unwrap().instructions.as_ref().unwrap().contains("aether-tool-server mcp"));
    assert!(client.list_all_tools().await.is_err(), "context-free discovery must not reveal tools");
    let a = policy(ToolFilter::default());
    let b = policy(ToolFilter::default());
    let mut params = PaginatedRequestParams::default();
    params.meta = Some(metadata(&a));
    let listing = client.list_tools(Some(params)).await.unwrap();
    assert!(listing.tools.iter().any(|tool| tool.name == "coding__read_file"));
    assert!(listing.tools.iter().all(|tool| tool.name.starts_with("coding__")));
    assert!(!listing.tools.iter().any(|tool| tool.name == "_aether_list_servers"));

    let read = CallToolRequestParams::new("coding__read_file")
        .with_arguments(json!({"filePath":"existing.txt"}).as_object().unwrap().clone());
    let mut read_a = read.clone();
    read_a.meta = Some(metadata(&a));
    assert!(!client.call_tool(read_a).await.unwrap().is_error.unwrap_or(false));

    let mut write = CallToolRequestParams::new("coding__write_file")
        .with_arguments(json!({"filePath":"existing.txt","content":"changed"}).as_object().unwrap().clone());
    write.meta = Some(metadata(&b));
    assert!(client.call_tool(write.clone()).await.unwrap().is_error.unwrap_or(false));
    assert_eq!(std::fs::read_to_string(workspace.path().join("existing.txt")).unwrap(), "original");
    write.meta = Some(metadata(&a));
    assert!(!client.call_tool(write).await.unwrap().is_error.unwrap_or(false));
    assert_eq!(std::fs::read_to_string(workspace.path().join("existing.txt")).unwrap(), "changed");

    let restricted = policy(serde_json::from_value(json!({"allow":["vm__coding__read_file"]})).unwrap());
    let mut params = PaginatedRequestParams::default();
    params.meta = Some(metadata(&restricted));
    let listing = client.list_tools(Some(params)).await.unwrap();
    assert_eq!(listing.tools.len(), 1);
    let mut guessed = CallToolRequestParams::new("coding__write_file")
        .with_arguments(json!({"filePath":"forbidden","content":"no"}).as_object().unwrap().clone());
    guessed.meta = Some(metadata(&restricted));
    assert!(client.call_tool(guessed).await.is_err());
    assert!(!workspace.path().join("forbidden").exists());
    client.cancel().await.unwrap();
    cancellation.cancel();
    server.await.unwrap().unwrap();
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn shutdown_closes_shared_gateway_even_with_live_runtime_clones() {
    let workspace = tempfile::tempdir().unwrap();
    let config = RemoteConfig::new(workspace.path(), McpConfig::default(), &Vars::new()).unwrap();
    let mut runtime = RemoteToolRuntime::new(config).await.unwrap();
    let other = runtime.clone();
    let endpoint = runtime.gateway_endpoint().unwrap().to_path_buf();
    runtime.shutdown().await.unwrap();
    assert!(tokio::net::UnixStream::connect(&endpoint).await.is_err());
    assert!(!endpoint.exists());
    drop(other);
}

#[tokio::test]
async fn transport_rejects_origins_hosts_and_large_bodies() {
    let workspace = tempfile::tempdir().unwrap();
    let config = RemoteConfig::new(workspace.path(), McpConfig::default(), &Vars::new()).unwrap();
    let runtime = RemoteToolRuntime::new(config).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let cancellation = CancellationToken::new();
    let options = HttpOptions { max_request_body_bytes: 128, ..HttpOptions::default() };
    let server = tokio::spawn(serve(listener, runtime, options, cancellation.clone()));
    let client = reqwest::Client::new();
    let url = format!("http://{address}/mcp");
    for (name, value) in [("Origin", "https://example.org"), ("Host", "evil.example")] {
        let response = client
            .post(&url)
            .header(name, value)
            .header("Content-Type", "application/json")
            .body("{}")
            .send()
            .await
            .unwrap();
        assert!(!response.status().is_success());
    }
    let response = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body("x".repeat(129))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::PAYLOAD_TOO_LARGE);
    cancellation.cancel();
    server.await.unwrap().unwrap();
}

fn policy(agent_tools: ToolFilter) -> GatewayRequestContext {
    GatewayRequestContext {
        identity: AgentIdentity::new(),
        execution_task: None,
        server_alias: "vm".into(),
        agent_tools,
        server_tools: ToolFilter::default(),
        defer_tools: ToolExposure::default(),
    }
}

fn metadata(policy: &GatewayRequestContext) -> RequestMetaObject {
    let mut meta = RequestMetaObject::default();
    policy.merge_into(&mut meta).unwrap();
    meta
}
