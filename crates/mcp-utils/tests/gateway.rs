use futures::{FutureExt, StreamExt};
use mcp_utils::client::{
    ClientError, ClientOptions, HeaderMap, HttpAuth, OAuthSettings, ToolCall, ToolCallOptions, Transport,
};
use mcp_utils::gateway::{ConnectedServer, GatewayError, McpGateway, ServerSpec};
use mcp_utils::model::{ContentBlock, GetPromptResult, Tool, ToolAnnotations};
use mcp_utils::server::{McpServer, UnixSocket};
use mcp_utils::testing::{FakeMcpServer, FakeMcpState, FakeTool, FakeToolResponse, args, elicitation_host, wait_until};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::sync::Weak;
use utils::mcp_status::McpServerStatus;

#[tokio::test]
async fn servers_are_connecting_until_their_handshake_completes() {
    let gateway = gateway(vec![stdio("silent", "sleep", &["60"])]);

    let statuses = gateway.statuses();
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].status, McpServerStatus::Connecting);
    gateway.shutdown().await;
}

#[tokio::test]
async fn ready_waits_until_every_server_connects_or_fails() {
    let gateway = gateway(vec![
        ServerSpec::in_process("math", FakeMcpServer::math()),
        stdio("broken", "aether-missing-mcp-server", &[]),
    ]);

    gateway.ready().await;
    let statuses = gateway.statuses();

    assert_eq!(statuses.iter().map(|entry| entry.name.as_str()).collect::<Vec<_>>(), ["math", "broken"]);
    assert_eq!(statuses[0].status, McpServerStatus::Connected { tool_count: 2 });
    assert!(matches!(&statuses[1].status, McpServerStatus::Failed { error } if error.contains("Failed to spawn")));
    assert!(!statuses[1].can_authenticate());
}

#[tokio::test]
async fn ready_resolves_immediately_without_servers() {
    let gateway = ready(gateway(Vec::new())).await;

    assert_eq!(gateway.statuses().len(), 0);
}

#[tokio::test]
async fn unreachable_http_servers_fail_without_offering_oauth() {
    let (options, _host) = elicitation_host();
    let gateway = ready(McpGateway::new(vec![unreachable_http("remote")], &options)).await;

    let status = &gateway.statuses()[0];
    assert!(matches!(status.status, McpServerStatus::Failed { .. }));
    assert!(!status.can_authenticate());
}

#[tokio::test]
async fn failing_servers_do_not_block_the_others() {
    let servers = vec![
        unreachable_http("failing"),
        ServerSpec::in_process("working", FakeMcpServer::new()),
        unreachable_http("also_failing"),
    ];
    let gateway = ready(gateway(servers)).await;

    let statuses = gateway.statuses();
    assert_eq!(gateway.servers().iter().map(ConnectedServer::name).collect::<Vec<_>>(), ["working"]);
    assert!(matches!(statuses[0].status, McpServerStatus::Failed { .. }));
    assert!(matches!(statuses[1].status, McpServerStatus::Connected { .. }));
    assert!(matches!(statuses[2].status, McpServerStatus::Failed { .. }));
}

#[tokio::test]
async fn servers_report_their_tools_as_the_server_named_them() {
    let server = FakeMcpServer::math()
        .with_tool(FakeTool::new("lookup").description("Looks up").annotations(ToolAnnotations::new().read_only(true)));
    let gateway = ready(gateway(vec![ServerSpec::in_process("math", server)])).await;

    let servers = gateway.servers();

    let tools = servers[0].tools();
    assert_eq!(names(tools), ["add_numbers", "divide_numbers", "lookup"]);
    let lookup = &tools[2];
    assert_eq!(lookup.description.as_deref(), Some("Looks up"));
    assert_eq!(Value::Object((*lookup.input_schema).clone()), json!({"type": "object", "properties": {}}));
    assert_eq!(lookup.annotations.as_ref().and_then(|annotations| annotations.read_only_hint), Some(true));
}

#[tokio::test]
async fn calls_reach_the_server_by_the_name_it_gave_the_tool() {
    let gateway = ready(gateway(vec![ServerSpec::in_process("math", FakeMcpServer::math())])).await;

    let result =
        gateway.servers()[0].call_tool("add_numbers", args(json!({"a": 2, "b": 3})), ToolCallOptions::default());

    assert_eq!(result.result().await.unwrap().structured_content, Some(json!({"sum": 5})));
}

#[tokio::test]
async fn tool_list_changes_refresh_the_tools() {
    let server = FakeMcpServer::math();
    let state = server.state();
    let gateway = ready(gateway(vec![ServerSpec::in_process("math", server)])).await;

    state.add_tool_and_notify(FakeTool::new("added_later")).await;
    wait_until(&gateway, |gateway| names(gateway.servers()[0].tools()).contains(&"added_later".to_string())).await;
    state.clear_tools_and_notify().await;
    wait_until(&gateway, |gateway| gateway.servers()[0].tools().is_empty()).await;

    assert_eq!(gateway.statuses()[0].status, McpServerStatus::Connected { tool_count: 0 });
}

#[tokio::test]
async fn failed_tool_refreshes_keep_the_previous_tools() {
    let server = FakeMcpServer::math();
    let state = server.state();
    let gateway = ready(gateway(vec![ServerSpec::in_process("math", server)])).await;

    fail_a_refresh(&state).await;
    fail_a_refresh(&state).await;

    assert_eq!(names(gateway.servers()[0].tools()), ["add_numbers", "divide_numbers"]);
    assert_eq!(gateway.statuses()[0].status, McpServerStatus::Connected { tool_count: 2 });
}

#[tokio::test]
async fn changes_report_only_what_happens_after_subscribing() {
    let server = FakeMcpServer::math();
    let state = server.state();
    let gateway = ready(gateway(vec![ServerSpec::in_process("math", server)])).await;
    let mut changes = gateway.changes();

    assert_eq!(changes.next().now_or_never(), None);
    state.add_tool_and_notify(FakeTool::new("added_later")).await;
    assert_eq!(changes.next().await, Some(()));
}

#[tokio::test]
async fn prompts_are_listed_with_their_server_and_expanded_by_it() {
    let docs = FakeMcpServer::new().with_prompt("greet", "Hello {name}");
    let gateway = ready(gateway(vec![
        ServerSpec::in_process("docs", docs),
        ServerSpec::in_process("math", FakeMcpServer::math()),
    ]))
    .await;

    let prompts = gateway.list_prompts().await;
    let expanded = gateway.get_prompt("docs", "greet", args(json!({"name": "Ada"}))).await.unwrap();

    let listed = prompts.iter().map(|listed| (listed.server.as_str(), listed.prompt.name.as_str())).collect::<Vec<_>>();
    assert_eq!(listed, [("docs", "greet")]);
    assert_eq!(prompt_text(&expanded), "Hello Ada");
    assert!(matches!(
        gateway.get_prompt("missing", "greet", Map::new()).await,
        Err(GatewayError::ServerNotFound(name)) if name == "missing"
    ));
}

#[tokio::test]
async fn authenticate_needs_a_known_oauth_server() {
    let gateway = ready(gateway(vec![ServerSpec::in_process("math", FakeMcpServer::math())])).await;

    assert!(
        matches!(gateway.authenticate("missing").await, Err(GatewayError::ServerNotFound(name)) if name == "missing")
    );
    assert!(
        matches!(gateway.authenticate("math").await, Err(GatewayError::Client(ClientError::OAuthUnavailable { server })) if server == "math")
    );
    assert!(!gateway.statuses()[0].can_authenticate());
}

#[tokio::test]
async fn shutdown_releases_in_process_servers_and_closes_the_gateway() {
    let server = FakeMcpServer::new();
    let alive = server.alive();
    let gateway = ready(gateway(vec![ServerSpec::in_process("probe", server)])).await;

    gateway.shutdown().await;

    assert!(alive.upgrade().is_none());
    assert_eq!(gateway.statuses().len(), 0);
    gateway.ready().await;
    assert_eq!(gateway.statuses().len(), 0);
    assert!(matches!(gateway.authenticate("probe").await, Err(GatewayError::Closed)));
    assert!(gateway.servers().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn shutdown_releases_in_process_servers_while_another_handle_is_held() {
    let server = FakeMcpServer::new();
    let alive = server.alive();
    let gateway = ready(gateway(vec![ServerSpec::in_process("probe", server)])).await;
    let _held = gateway.clone();

    gateway.shutdown().await;

    released(&alive).await;
}

#[tokio::test]
async fn dropping_every_handle_releases_in_process_servers() {
    let server = FakeMcpServer::new();
    let alive = server.alive();
    let gateway = ready(gateway(vec![ServerSpec::in_process("probe", server)])).await;
    let mut changes = gateway.changes();

    drop(gateway);
    while changes.next().await.is_some() {}

    released(&alive).await;
}

#[tokio::test]
async fn shutdown_cancels_calls_in_flight() {
    let server = slow_server();
    let state = server.state();
    let gateway = ready(gateway(vec![ServerSpec::in_process("slow", server)])).await;
    let in_flight = tokio::spawn(slow_call(&gateway.servers()[0]).result());
    started(&state).await;

    gateway.shutdown().await;

    assert!(in_flight.await.unwrap().is_err());
}

#[tokio::test]
async fn dropping_every_handle_stops_the_gateway_and_cancels_calls_in_flight() {
    let server = slow_server();
    let state = server.state();
    let gateway = ready(gateway(vec![ServerSpec::in_process("slow", server)])).await;
    let in_flight = tokio::spawn(slow_call(&gateway.servers()[0]).result());
    let mut changes = gateway.changes();
    started(&state).await;

    drop(gateway);
    while changes.next().await.is_some() {}

    assert!(in_flight.await.unwrap().is_err());
}

#[tokio::test]
async fn refreshing_an_unchanged_tool_list_reports_no_change() {
    let server = FakeMcpServer::math();
    let state = server.state();
    let gateway = ready(gateway(vec![ServerSpec::in_process("math", server)])).await;
    let mut changes = gateway.changes();

    refresh(&state).await;
    state.fail_next_tool_list();
    refresh(&state).await;

    assert!(changes.next().now_or_never().is_none(), "an unchanged tool list was reported as a change");
}

#[tokio::test]
async fn a_server_that_disconnects_is_reported_as_failed() {
    let socket = UnixSocket::bind().unwrap();
    let transport = Transport::Unix(socket.path().to_path_buf());
    let server = McpServer::new(FakeMcpServer::math()).serve_unix(socket);
    let gateway = ready(gateway(vec![ServerSpec { name: "math".to_string(), transport }])).await;
    assert_eq!(gateway.servers().len(), 1);

    server.stop();
    wait_until(&gateway, |gateway| matches!(gateway.statuses()[0].status, McpServerStatus::Failed { .. })).await;

    assert!(gateway.servers().is_empty());
    assert!(!gateway.statuses()[0].can_authenticate());
}

fn gateway(servers: Vec<ServerSpec>) -> McpGateway {
    McpGateway::new(servers, &ClientOptions::default())
}

async fn ready(gateway: McpGateway) -> McpGateway {
    gateway.ready().await;
    gateway
}

fn stdio(name: &str, command: &str, args: &[&str]) -> ServerSpec {
    let args = args.iter().map(ToString::to_string).collect();
    let transport = Transport::Stdio { command: command.to_string(), args, env: HashMap::new(), cwd: None };
    ServerSpec { name: name.to_string(), transport }
}

fn unreachable_http(name: &str) -> ServerSpec {
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let url = format!("http://127.0.0.1:{port}/mcp");
    let transport = Transport::Http { url, headers: HeaderMap::new(), auth: HttpAuth::OAuth(OAuthSettings::default()) };
    ServerSpec { name: name.to_string(), transport }
}

fn slow_server() -> FakeMcpServer {
    FakeMcpServer::new().with_tool(FakeTool::new("slow").responds(FakeToolResponse::pending()))
}

fn slow_call(server: &ConnectedServer) -> ToolCall {
    server.call_tool("slow", Map::new(), ToolCallOptions::default())
}

async fn started(state: &FakeMcpState) {
    state.wait_until(|state| !state.calls_for("slow").is_empty()).await;
}

async fn refresh(state: &FakeMcpState) {
    let requests = state.tool_list_requests();
    state.notify_tools_changed().await;
    state.wait_until(|state| state.tool_list_requests() > requests).await;
}

async fn fail_a_refresh(state: &FakeMcpState) {
    let requests = state.tool_list_requests();
    state.fail_next_tool_list();
    state.clear_tools_and_notify().await;
    state.wait_until(|state| state.tool_list_requests() > requests).await;
}

fn prompt_text(prompt: &GetPromptResult) -> &str {
    let ContentBlock::Text(text) = &prompt.messages[0].content else { panic!("expected text") };
    &text.text
}

fn names(tools: &[Tool]) -> Vec<String> {
    tools.iter().map(|tool| tool.name.to_string()).collect()
}

async fn released(alive: &Weak<()>) {
    while alive.upgrade().is_some() {
        tokio::task::yield_now().await;
    }
}
