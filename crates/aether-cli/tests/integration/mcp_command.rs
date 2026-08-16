use mcp_utils::tool_gateway::{AETHER_MCP_IPC_SOCKET, LIST_SERVERS_TOOL, ServerDescription, UnixSocketMcpTransport};
use rmcp::{
    RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ErrorData, Implementation, ListToolsResult,
        PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool,
    },
    service::RequestContext,
};
use serde_json::{Map, Value, json};
use std::ffi::OsString;
use std::io::Write;
use std::process::{Command, Output, Stdio};
use std::sync::Arc;

#[tokio::test(flavor = "multi_thread")]
async fn command_requires_an_inherited_runtime() {
    let output = run_standalone(&["mcp", "--help"]).await;

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("only available inside an active Aether session"));
}

#[tokio::test(flavor = "multi_thread")]
async fn inherited_runtime_supports_discovery_execution_and_usage_errors() {
    let transport = UnixSocketMcpTransport::bind().unwrap();
    let environment = transport.environment();
    let _handle = transport.start(FakeGateway);

    let help = run(&environment, &["mcp", "--help"]).await;
    assert!(help.status.success(), "{}", String::from_utf8_lossy(&help.stderr));
    assert!(String::from_utf8_lossy(&help.stdout).contains("fake"));

    let execution = run(&environment, &["mcp", "fake", "echo", "value=\"hello\""]).await;
    assert!(execution.status.success(), "{}", String::from_utf8_lossy(&execution.stderr));
    assert_eq!(serde_json::from_slice::<Value>(&execution.stdout).unwrap(), json!({"value": "hello"}));

    let usage = run(&environment, &["mcp", "fake", "echo", "value=1", "value=2"]).await;
    assert_eq!(usage.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&usage.stderr).contains("duplicate field 'value'"));
}

#[tokio::test(flavor = "multi_thread")]
async fn tool_arguments_accept_every_input_form() {
    let transport = UnixSocketMcpTransport::bind().unwrap();
    let environment = transport.environment();
    let _handle = transport.start(FakeGateway);

    let flags = run(&environment, &["mcp", "fake", "echo", "--value", "hello", "--count", "2"]).await;
    assert_eq!(arguments(&flags), json!({"value": "hello", "count": 2}));

    let object = run(&environment, &["mcp", "fake", "echo", "--args", r#"{"value":"hello"}"#]).await;
    assert_eq!(arguments(&object), json!({"value": "hello"}));

    let escaped = run(&environment, &["mcp", "fake", "echo", "--", "value=hello"]).await;
    assert_eq!(arguments(&escaped), json!({"value": "hello"}));

    let piped = run_with_stdin(&environment, &["mcp", "fake", "echo"], r#"{"value":"hello"}"#).await;
    assert_eq!(arguments(&piped), json!({"value": "hello"}));

    let empty = run(&environment, &["mcp", "fake", "echo"]).await;
    assert_eq!(arguments(&empty), json!({}));
}

#[tokio::test(flavor = "multi_thread")]
async fn conflicting_or_malformed_arguments_are_usage_errors() {
    let transport = UnixSocketMcpTransport::bind().unwrap();
    let environment = transport.environment();
    let _handle = transport.start(FakeGateway);

    let cases = [
        (vec!["mcp", "fake", "echo", "--value"], "--value requires a value"),
        (vec!["mcp", "fake", "echo", "value"], "expected key=value or --key value, got 'value'"),
        (vec!["mcp", "fake", "echo", "=hello"], "field name cannot be empty"),
        (vec!["mcp", "fake", "echo", "--args", "[]"], "--args must contain a JSON object"),
        (vec!["mcp", "fake", "echo", "--args", "{}", "value=1"], "mutually exclusive"),
    ];
    for (args, message) in cases {
        let output = run(&environment, &args).await;
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        assert_eq!(output.status.code(), Some(2), "{args:?} produced {stderr}");
        assert!(stderr.contains(message), "{args:?} produced {stderr}");
    }

    let conflict = run_with_stdin(&environment, &["mcp", "fake", "echo", "value=1"], r#"{"value":2}"#).await;
    assert_eq!(conflict.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&conflict.stderr).contains("mutually exclusive"));
}

fn arguments(output: &Output) -> Value {
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    serde_json::from_slice(&output.stdout).unwrap()
}

async fn run_standalone(args: &[&str]) -> Output {
    let args = args.iter().map(ToString::to_string).collect::<Vec<_>>();
    tokio::task::spawn_blocking(move || {
        Command::new(env!("CARGO_BIN_EXE_aether")).args(args).env_remove(AETHER_MCP_IPC_SOCKET).output().unwrap()
    })
    .await
    .unwrap()
}

async fn run(environment: &[(OsString, OsString)], args: &[&str]) -> Output {
    run_with_stdin(environment, args, "").await
}

async fn run_with_stdin(environment: &[(OsString, OsString)], args: &[&str], stdin: &str) -> Output {
    let socket = env_value(environment, AETHER_MCP_IPC_SOCKET);
    let args = args.iter().map(ToString::to_string).collect::<Vec<_>>();
    let stdin = stdin.to_string();
    tokio::task::spawn_blocking(move || {
        let mut child = Command::new(env!("CARGO_BIN_EXE_aether"))
            .args(args)
            .env(AETHER_MCP_IPC_SOCKET, socket)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
        child.wait_with_output().unwrap()
    })
    .await
    .unwrap()
}

#[derive(Clone)]
struct FakeGateway;

impl ServerHandler for FakeGateway {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("fake-gateway", "1"))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(vec![
            Tool::new(LIST_SERVERS_TOOL, "List servers", Arc::new(Map::from_iter([("type".into(), json!("object"))]))),
            Tool::new("fake__echo", "Echo input", Arc::new(Map::from_iter([("type".into(), json!("object"))]))),
        ]))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if request.name == LIST_SERVERS_TOOL {
            return Ok(CallToolResult::structured(
                serde_json::to_value(vec![ServerDescription { name: "fake".into(), description: "Fake tools".into() }])
                    .unwrap(),
            )
            .into());
        }
        Ok(CallToolResult::structured(Value::Object(request.arguments.unwrap_or_default())).into())
    }
}

fn env_value(environment: &[(OsString, OsString)], key: &str) -> OsString {
    environment.iter().find(|(name, _)| name == key).unwrap().1.clone()
}
