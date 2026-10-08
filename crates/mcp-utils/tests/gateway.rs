use futures::StreamExt;
use mcp_utils::McpError;
use mcp_utils::client::{
    ClientOptions, HeaderMap, McpClient, ToolCallError, ToolCallEvent, ToolCallOptions, Transport,
};
use mcp_utils::gateway::{
    LIST_SERVERS_TOOL, McpGateway, ServerSpec, ToolAnnotationMatcher, ToolExposure, ToolFilter, ToolMatcher,
};
use mcp_utils::model::{
    ContentBlock, CreateTaskResult, DetailedTask, ServerCapabilities, ServerConfig, Task, TaskPayload, TaskStatus,
    Tool, ToolAnnotations,
};
use mcp_utils::server::McpServer;
use mcp_utils::testing::{FakeMcpServer, FakeTool, FakeToolResponse, args, connect, fake_mcp};
use rmcp::ServerHandler;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Weak};
use std::time::Duration;
use tokio::sync::{Notify, mpsc};
use utils::mcp_status::{McpServerAuthCapability, McpServerStatus};

#[tokio::test]
async fn servers_are_connecting_until_their_handshake_completes() {
    let gateway = gateway();

    gateway.add_servers(vec![stdio("silent", "sleep", &["60"])]).unwrap();

    let statuses = gateway.catalog().statuses();
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].status, McpServerStatus::Connecting);
    gateway.shutdown().await;
}

#[tokio::test]
async fn ready_waits_until_every_server_connects_or_fails() {
    let gateway = gateway();
    gateway
        .add_servers(vec![fake_mcp("math", FakeMcpServer::new()), stdio("broken", "aether-missing-mcp-server", &[])])
        .unwrap();

    let statuses = gateway.ready().await.statuses();

    assert_eq!(statuses.iter().map(|entry| entry.name.as_str()).collect::<Vec<_>>(), ["math", "broken"]);
    assert_eq!(statuses[0].status, McpServerStatus::Connected { tool_count: 3 });
    assert!(matches!(&statuses[1].status, McpServerStatus::Failed { error } if error.contains("Failed to spawn")));
    assert!(!statuses[1].can_authenticate());
}

#[tokio::test]
async fn ready_resolves_immediately_without_servers() {
    let gateway = gateway();

    assert!(gateway.ready().await.statuses().is_empty());
}

#[tokio::test]
async fn re_adding_a_server_replaces_its_connection() {
    let gateway = ready(gateway(), vec![fake_mcp("math", McpServer::new(Probe::default()))]).await;

    gateway.add_servers(vec![fake_mcp("math", FakeMcpServer::new())]).unwrap();

    let statuses = gateway.ready().await.statuses();
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].status, McpServerStatus::Connected { tool_count: 3 });
    let result =
        gateway.call_tool("math__add_numbers", args(json!({"a": 2, "b": 3})), ToolCallOptions::default()).unwrap();
    assert_eq!(result.result().await.unwrap().structured_content, Some(json!({"sum": 5})));
}

#[tokio::test]
async fn unreachable_http_servers_fail_without_offering_oauth() {
    let (events, _host) = mpsc::channel(1);
    let gateway = McpGateway::new(ClientOptions::default().elicitation(events), ToolFilter::default());
    let gateway = ready(gateway, vec![unreachable_http("remote", ToolExposure::ModelVisible)]).await;

    let status = &gateway.catalog().statuses()[0];
    assert!(matches!(status.status, McpServerStatus::Failed { .. }));
    assert_eq!(status.auth_capability, McpServerAuthCapability::Unavailable);
    assert!(!status.can_authenticate());
}

#[tokio::test]
async fn failing_servers_do_not_block_the_others() {
    let servers = vec![
        unreachable_http("failing", ToolExposure::ModelVisible),
        fake_mcp("working", FakeMcpServer::new()),
        with_exposure(fake_mcp("deferred", FakeMcpServer::new()), ToolExposure::deferred_all()),
        unreachable_http("failing_deferred", ToolExposure::deferred_all()),
    ];
    let catalog = ready(gateway(), servers).await.catalog();

    let statuses = catalog.statuses();
    assert!(names(&catalog.tools()).iter().all(|name| name.starts_with("working__")));
    assert!(matches!(statuses[0].status, McpServerStatus::Failed { .. }));
    assert!(matches!(statuses[1].status, McpServerStatus::Connected { .. }));
    assert!(matches!(statuses[2].status, McpServerStatus::Connected { .. }));
    assert!(statuses[2].deferred_tools);
    assert!(matches!(statuses[3].status, McpServerStatus::Failed { .. }));
    assert!(statuses[3].deferred_tools);
}

#[tokio::test]
async fn re_adding_a_failed_server_reconnects_it_with_its_policy() {
    let selective = ToolExposure::Deferred(ToolFilter { allow: Vec::new(), deny: vec![ToolMatcher::name("add_*")] });
    let gateway = ready(gateway(), vec![unreachable_http("remote", selective.clone())]).await;

    let authenticate = gateway.authenticate("remote").await;
    gateway.add_servers(vec![with_exposure(fake_mcp("remote", FakeMcpServer::new()), selective)]).unwrap();
    let reconnected = gateway.ready().await;

    assert!(matches!(authenticate, Err(McpError::OAuthUnavailable { server }) if server == "remote"));
    let remote = &reconnected.statuses()[0];
    assert!(matches!(remote.status, McpServerStatus::Connected { .. }));
    assert!(remote.deferred_tools);
    assert_eq!(names(&reconnected.tools()), ["remote__add_numbers"]);
}

#[tokio::test]
async fn model_visible_tools_are_namespaced_and_keep_their_metadata() {
    let server = FakeMcpServer::new()
        .with_tool(FakeTool::new("lookup").description("Looks up").annotations(ToolAnnotations::new().read_only(true)));
    let gateway = ready(gateway(), vec![fake_mcp("math", server)]).await;

    let tools = gateway.catalog().tools();

    assert_eq!(names(&tools), ["math__add_numbers", "math__divide_numbers", "math__lookup", "math__slow_tool"]);
    let lookup = &tools[2];
    assert_eq!(lookup.description.as_deref(), Some("Looks up"));
    assert_eq!(Value::Object((*lookup.input_schema).clone()), json!({"type": "object", "properties": {}}));
    assert_eq!(lookup.annotations.as_ref().and_then(|annotations| annotations.read_only_hint), Some(true));
}

#[tokio::test]
async fn calls_reach_the_server_by_namespaced_name() {
    let gateway = ready(gateway(), vec![fake_mcp("math", FakeMcpServer::new())]).await;

    let result =
        gateway.call_tool("math__add_numbers", args(json!({"a": 2, "b": 3})), ToolCallOptions::default()).unwrap();

    assert_eq!(result.result().await.unwrap().structured_content, Some(json!({"sum": 5})));
}

#[tokio::test]
async fn unknown_tools_fail_without_reaching_a_server() {
    let gateway = ready(gateway(), vec![fake_mcp("math", FakeMcpServer::new())]).await;

    for name in ["math__missing", "other__add_numbers", "add_numbers"] {
        let result = gateway.call_tool(name, Map::new(), ToolCallOptions::default());

        assert!(matches!(&result, Err(McpError::ToolNotFound(tool)) if tool == name), "{:?}", result.err());
    }
}

#[tokio::test]
async fn deferred_tools_are_hidden_from_the_model_and_served_by_the_deferred_endpoint() {
    let exposure =
        ToolExposure::Deferred(ToolFilter { allow: vec![ToolMatcher::name("divide_numbers")], deny: Vec::new() });
    let gateway = ready(gateway(), vec![with_exposure(fake_mcp("math", FakeMcpServer::new()), exposure)]).await;
    let endpoint = connect("aether", gateway.deferred_tools_server(None), &ClientOptions::default()).await;

    let model_visible = names(&gateway.catalog().tools());
    let deferred = endpoint.list_tools().await.unwrap();
    let servers = call(&endpoint, LIST_SERVERS_TOOL, json!({})).await.unwrap();
    let quotient = call(&endpoint, "math__divide_numbers", json!({"a": 8, "b": 2})).await.unwrap();
    let direct_via_endpoint = call(&endpoint, "math__add_numbers", json!({"a": 1, "b": 2})).await.unwrap_err();
    let deferred_via_model = gateway.call_tool("math__divide_numbers", Map::new(), ToolCallOptions::default());

    assert_eq!(model_visible, ["math__add_numbers", "math__slow_tool"]);
    assert_eq!(
        deferred.iter().map(|tool| tool.name.as_ref()).collect::<Vec<_>>(),
        ["math__divide_numbers", LIST_SERVERS_TOOL]
    );
    assert_eq!(deferred[0].description.as_deref(), Some("Divides two numbers"));
    assert_eq!(
        servers.structured_content,
        Some(json!([{"name": "math", "description": "A fake MCP server for testing"}]))
    );
    assert_eq!(quotient.structured_content, Some(json!({"quotient": 4})));
    assert!(direct_via_endpoint.to_string().contains("exposed directly; call 'math__add_numbers'"));
    assert!(matches!(deferred_via_model, Err(McpError::ToolNotFound(_))));
    assert!(gateway.catalog().statuses()[0].deferred_tools);
}

#[tokio::test]
async fn disconnecting_from_the_deferred_endpoint_cancels_the_server_task() {
    let working = || Task::new("task-1", TaskStatus::Working, now(), now()).with_poll_interval_ms(10);
    let server = FakeMcpServer::new()
        .with_tool(FakeTool::new("deferred").responds(FakeToolResponse::task(CreateTaskResult::new(working()))))
        .with_task("task-1", [DetailedTask::new(working(), TaskPayload::Working)]);
    let state = server.state();
    let gateway = ready(gateway(), vec![with_exposure(fake_mcp("lazy", server), ToolExposure::deferred_all())]).await;
    let endpoint = connect("aether", gateway.deferred_tools_server(None), &ClientOptions::default()).await;
    let caller = endpoint.clone();
    let in_flight = tokio::spawn(async move { call(&caller, "lazy__deferred", json!({})).await });
    state.wait_until(|state| !state.task_get_ids().is_empty()).await;

    endpoint.close().await;

    assert!(in_flight.await.unwrap().is_err());

    state.wait_until(|state| state.task_cancel_ids() == ["task-1"]).await;
}

#[tokio::test]
async fn the_tool_filter_hides_tools_from_the_model_and_the_deferred_endpoint() {
    let server =
        FakeMcpServer::new().with_tool(FakeTool::new("peek").annotations(ToolAnnotations::new().read_only(true)));
    let filter = ToolFilter {
        allow: Vec::new(),
        deny: vec![
            ToolMatcher::name("direct__divide_*"),
            ToolMatcher::annotations(ToolAnnotationMatcher {
                read_only: Some(true),
                ..ToolAnnotationMatcher::default()
            }),
        ],
    };
    let servers =
        vec![fake_mcp("direct", server.clone()), with_exposure(fake_mcp("lazy", server), ToolExposure::deferred_all())];
    let gateway = ready(McpGateway::new(ClientOptions::default(), filter), servers).await;
    let endpoint = connect("aether", gateway.deferred_tools_server(None), &ClientOptions::default()).await;

    let deferred = endpoint.list_tools().await.unwrap();
    let filtered = call(&endpoint, "lazy__peek", json!({})).await.unwrap_err();

    assert_eq!(names(&gateway.catalog().tools()), ["direct__add_numbers", "direct__slow_tool"]);
    assert!(!deferred.iter().any(|tool| tool.name == "lazy__peek"));
    assert!(filtered.to_string().contains("Tool not found: lazy__peek"));
    let statuses = gateway.catalog().statuses();
    assert_eq!(statuses[0].status, McpServerStatus::Connected { tool_count: 4 });
    assert!(!statuses[0].deferred_tools && statuses[1].deferred_tools);
}

#[tokio::test]
async fn deferred_tools_are_reported_but_their_instructions_are_not() {
    let direct = || fake_mcp("direct", FakeMcpServer::new());

    let without_deferred = ready(gateway(), vec![direct()]).await.catalog();
    let with_deferred = ready(
        gateway(),
        vec![direct(), with_exposure(fake_mcp("lazy", FakeMcpServer::new()), ToolExposure::deferred_all())],
    )
    .await
    .catalog();

    assert!(!without_deferred.has_deferred_tools());
    assert!(with_deferred.has_deferred_tools());
    assert_eq!(with_deferred.instructions().keys().collect::<Vec<_>>(), ["direct"]);
}

#[tokio::test]
async fn tool_list_changes_refresh_the_catalog() {
    let server = FakeMcpServer::new();
    let state = server.state();
    let gateway = ready(gateway(), vec![fake_mcp("math", server)]).await;
    let mut catalog = gateway.subscribe();

    state.add_tool_and_notify(FakeTool::new("added_later")).await;
    catalog.wait_for(|catalog| names(&catalog.tools()).contains(&"math__added_later".to_string())).await.unwrap();
    state.clear_tools_and_notify().await;
    let cleared = catalog.wait_for(|catalog| catalog.tools().is_empty()).await.unwrap().clone();

    assert!(cleared.instructions().is_empty());
    assert_eq!(cleared.statuses()[0].status, McpServerStatus::Connected { tool_count: 0 });
}

#[tokio::test]
async fn prompts_are_namespaced_and_expanded_by_their_server() {
    let docs = FakeMcpServer::new().with_prompt("greet", "Hello {name}");
    let gateway = ready(gateway(), vec![fake_mcp("docs", docs), fake_mcp("math", FakeMcpServer::new())]).await;

    let prompts = gateway.list_prompts().await.unwrap();
    let expanded = gateway.get_prompt("docs__greet", args(json!({"name": "Ada"}))).await.unwrap();

    assert_eq!(prompts.iter().map(|prompt| prompt.name.as_str()).collect::<Vec<_>>(), ["docs__greet"]);
    let ContentBlock::Text(text) = &expanded.messages[0].content else { panic!("expected text") };
    assert_eq!(text.text, "Hello Ada");
    assert!(matches!(
        gateway.get_prompt("missing__greet", Map::new()).await,
        Err(McpError::ServerNotFound(server)) if server == "missing"
    ));
    assert!(matches!(gateway.get_prompt("greet", Map::new()).await, Err(McpError::NotNamespaced(_))));
}

#[tokio::test]
async fn authenticate_needs_a_known_oauth_server() {
    let gateway = ready(gateway(), vec![fake_mcp("math", FakeMcpServer::new())]).await;

    assert!(matches!(gateway.authenticate("missing").await, Err(McpError::ServerNotFound(name)) if name == "missing"));
    assert!(
        matches!(gateway.authenticate("math").await, Err(McpError::OAuthUnavailable { server }) if server == "math")
    );
    assert_eq!(gateway.catalog().statuses()[0].auth_capability, McpServerAuthCapability::Unavailable);
}

#[tokio::test]
async fn shutdown_releases_in_process_servers_and_closes_the_gateway() {
    let probe = Probe::default();
    let alive = probe.alive();
    let gateway = ready(gateway(), vec![fake_mcp("probe", McpServer::new(probe))]).await;

    gateway.shutdown().await;

    assert!(alive.upgrade().is_none());
    assert!(gateway.catalog().statuses().is_empty());
    assert!(gateway.ready().await.statuses().is_empty());
    assert!(matches!(gateway.add_servers(Vec::new()), Err(McpError::GatewayClosed)));
    assert!(matches!(
        gateway.call_tool("probe__any", Map::new(), ToolCallOptions::default()),
        Err(McpError::GatewayClosed)
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn shutdown_releases_in_process_servers_while_a_catalog_is_still_held() {
    let probe = Probe::default();
    let alive = probe.alive();
    let gateway = ready(gateway(), vec![fake_mcp("probe", McpServer::new(probe))]).await;
    let _held = gateway.catalog();

    gateway.shutdown().await;

    assert!(alive.upgrade().is_none());
}

#[tokio::test]
async fn dropping_every_handle_releases_in_process_servers() {
    let probe = Probe::default();
    let alive = probe.alive();
    let gateway = ready(gateway(), vec![fake_mcp("probe", McpServer::new(probe))]).await;
    let mut catalog = gateway.subscribe();

    drop(gateway);
    while catalog.changed().await.is_ok() {}

    assert!(alive.upgrade().is_none());
    assert!(catalog.borrow().statuses().is_empty());
}

#[tokio::test]
async fn shutdown_cancels_calls_in_flight() {
    let (server, started) = blocking_server();
    let gateway = ready(gateway(), vec![fake_mcp("slow", server)]).await;
    let in_flight =
        tokio::spawn(gateway.call_tool("slow__block", Map::new(), ToolCallOptions::default()).unwrap().result());
    started.notified().await;

    gateway.shutdown().await;

    assert!(in_flight.await.unwrap().is_err());
}

#[tokio::test]
async fn dropping_every_handle_stops_the_gateway_and_cancels_calls_in_flight() {
    let (server, started) = blocking_server();
    let gateway = ready(gateway(), vec![fake_mcp("slow", server)]).await;
    let endpoint = connect("aether", gateway.deferred_tools_server(None), &ClientOptions::default()).await;
    let in_flight =
        tokio::spawn(gateway.call_tool("slow__block", Map::new(), ToolCallOptions::default()).unwrap().result());
    let mut catalog = gateway.subscribe();
    started.notified().await;

    drop(gateway);
    while catalog.changed().await.is_ok() {}

    assert!(in_flight.await.unwrap().is_err());
    let error = call(&endpoint, "slow__block", json!({})).await.unwrap_err();
    assert!(error.to_string().contains("MCP gateway is closed"), "{error}");
}

#[tokio::test]
async fn the_deferred_endpoint_relays_upstream_progress() {
    let ci = FakeMcpServer::new()
        .with_tool(FakeTool::new("build").responds(FakeToolResponse::text("built").progress_message(1.0, "compiling")));
    let gateway = ready(gateway(), vec![with_exposure(fake_mcp("ci", ci), ToolExposure::deferred_all())]).await;
    let endpoint = connect("aether", gateway.deferred_tools_server(None), &ClientOptions::default()).await;

    let events = endpoint.call_tool("ci__build", Map::new(), ToolCallOptions::default()).collect::<Vec<_>>().await;

    assert!(events.iter().any(|event| matches!(
        event,
        ToolCallEvent::Progress(progress) if progress.message.as_deref() == Some("compiling")
    )));
    assert!(matches!(events.last(), Some(ToolCallEvent::Done { result: Ok(_), .. })));
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn gateway() -> McpGateway {
    McpGateway::new(ClientOptions::default(), ToolFilter::default())
}

async fn ready(gateway: McpGateway, servers: Vec<ServerSpec>) -> McpGateway {
    gateway.add_servers(servers).expect("servers are valid");
    gateway.ready().await;
    gateway
}

fn with_exposure(spec: ServerSpec, exposure: ToolExposure) -> ServerSpec {
    ServerSpec { exposure, ..spec }
}

fn stdio(name: &str, command: &str, args: &[&str]) -> ServerSpec {
    let args = args.iter().map(ToString::to_string).collect();
    let transport = Transport::Stdio { command: command.to_string(), args, env: HashMap::new() };
    ServerSpec { name: name.to_string(), transport, exposure: ToolExposure::ModelVisible }
}

fn unreachable_http(name: &str, exposure: ToolExposure) -> ServerSpec {
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let transport =
        Transport::Http { url: format!("http://127.0.0.1:{port}/mcp"), headers: HeaderMap::new(), oauth: None };
    ServerSpec { name: name.to_string(), transport, exposure }
}

fn blocking_server() -> (FakeMcpServer, Arc<Notify>) {
    let started = Arc::new(Notify::new());
    let signal = Arc::clone(&started);
    let tool = FakeTool::new("block").responds_with(move |_| {
        signal.notify_one();
        FakeToolResponse::text("too late").delay(Duration::from_hours(1))
    });
    (FakeMcpServer::new().with_tool(tool), started)
}

async fn call(
    client: &McpClient,
    tool: &str,
    arguments: Value,
) -> Result<mcp_utils::model::CallToolResult, ToolCallError> {
    client.call_tool(tool, args(arguments), ToolCallOptions::default()).result().await
}

fn names(tools: &[Tool]) -> Vec<String> {
    tools.iter().map(|tool| tool.name.to_string()).collect()
}

#[derive(Default)]
struct Probe {
    alive: Arc<()>,
}

impl Probe {
    fn alive(&self) -> Weak<()> {
        Arc::downgrade(&self.alive)
    }
}

impl ServerHandler for Probe {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }
}
