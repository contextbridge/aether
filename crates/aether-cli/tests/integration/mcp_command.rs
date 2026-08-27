use mcp_servers::coding::tools::bash::{BashEnvironment, BashInput, execute_command};
use mcp_utils::tool_gateway::{AETHER_MCP_IPC_SOCKET, UnixSocketMcpTransport, UnixSocketPath};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ErrorData, Implementation, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool,
};
use rmcp::{RoleServer, ServerHandler, service::RequestContext};
use serde_json::{Map, json};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::sync::Arc;

#[derive(Clone)]
struct FakeGateway;

impl ServerHandler for FakeGateway {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("fake-gateway", "1.0.0"))
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, ErrorData>> + Send + '_ {
        let schema = json!({
            "type": "object",
            "properties": { "message": { "type": "string", "description": "Text to echo" } },
            "required": ["message"]
        });
        std::future::ready(Ok(ListToolsResult::with_all_items(vec![
            Tool::new("_aether_list_servers", "List deferred servers", Arc::new(Map::new())),
            Tool::new("demo__echo", "Echo a message", Arc::new(schema.as_object().unwrap().clone())),
            Tool::new("demo__fail", "Return a tool error", Arc::new(Map::new())),
            Tool::new("demo__slow", "Wait for cancellation", Arc::new(Map::new())),
            Tool::new("demo__text", "Return content blocks", Arc::new(Map::new())),
        ])))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        match request.name.as_ref() {
            "_aether_list_servers" => Ok(CallToolResult::structured(json!([
                {"name": "demo", "description": "Demonstration tools"}
            ]))
            .into()),
            "demo__echo" => Ok(CallToolResult::structured(json!({
                "echoed": request.arguments.unwrap_or_default().get("message").cloned().unwrap_or(json!(null))
            }))
            .into()),
            "demo__fail" => Ok(CallToolResult::error(vec![]).into()),
            "demo__slow" => {
                tokio::time::sleep(std::time::Duration::from_mins(1)).await;
                Ok(CallToolResult::structured(json!({"done": true})).into())
            }
            "demo__text" => Ok(CallToolResult::success(vec![ContentBlock::text("plain")]).into()),
            _ => Err(ErrorData::invalid_params("unknown tool", None)),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn layered_help_reveals_servers_tools_and_complete_schema() {
    let (_server, socket) = gateway();

    let top = run(&socket, &["mcp", "--help"], None);
    assert_success(&top);
    assert_stdout_contains(&top, "demo");
    assert_stdout_contains(&top, "Demonstration tools");

    let server = run(&socket, &["mcp", "demo", "--help"], None);
    assert_success(&server);
    assert_stdout_contains(&server, "echo");
    assert_stdout_contains(&server, "Echo a message");

    let tool = run(&socket, &["mcp", "demo", "echo", "--help"], None);
    assert_success(&tool);
    assert_stdout_contains(&tool, "Text to echo");
    assert_stdout_contains(&tool, "\"required\"");
    assert_stdout_contains(&tool, "--json");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn calls_accept_empty_json_flag_or_stdin_and_print_one_json_value() {
    let (_server, socket) = gateway();

    let empty = run(&socket, &["mcp", "demo", "echo"], None);
    assert_success(&empty);
    assert_eq!(stdout_json(&empty), json!({"echoed": null}));

    let flag = run(&socket, &["mcp", "demo", "echo", "--json", r#"{"message":"flag"}"#], None);
    assert_success(&flag);
    assert_eq!(stdout_json(&flag), json!({"echoed": "flag"}));

    let stdin = run(&socket, &["mcp", "demo", "echo"], Some(r#"{"message":"stdin"}"#));
    assert_success(&stdin);
    assert_eq!(stdout_json(&stdin), json!({"echoed": "stdin"}));

    let content = run(&socket, &["mcp", "demo", "text"], None);
    assert_success(&content);
    assert_eq!(stdout_json(&content)[0]["text"], "plain");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn json_flag_does_not_wait_for_open_stdin() {
    let (_server, socket) = gateway();
    let mut command = Command::new(env!("CARGO_BIN_EXE_aether"));
    command
        .args(["mcp", "demo", "echo", "--json", r#"{"message":"flag"}"#])
        .env(AETHER_MCP_IPC_SOCKET, socket)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    let _open_stdin = child.stdin.take().unwrap();
    let output = child.wait_with_output().unwrap();

    assert_success(&output);
    assert_eq!(stdout_json(&output), json!({"echoed": "flag"}));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn coding_bash_composes_real_cli_with_jq_pipes_redirects_and_scripts() {
    let (_server, socket) = gateway();
    let workspace = tempfile::tempdir().unwrap();
    let script = workspace.path().join("call.sh");
    std::fs::write(
        &script,
        "#!/bin/bash\nset -e\naether mcp demo echo --json '{\"message\":\"script\"}' | jq -r .echoed\n",
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&script).unwrap().permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&script, permissions).unwrap();

    let binary_directory = Path::new(env!("CARGO_BIN_EXE_aether")).parent().unwrap();
    let environment =
        BashEnvironment::new().with_path_prepend(binary_directory).with_var(AETHER_MCP_IPC_SOCKET, &socket);
    assert_eq!(environment.vars().len(), 2, "only PATH and the session socket are injected");

    let output = execute_command(
        BashInput {
            command: concat!(
                "printf '%s' '{\"message\":\"pipe\"}' | aether mcp demo echo | jq -r .echoed > pipe.txt && ",
                "aether mcp demo echo --json '{\"message\":\"redirect\"}' > result.json && ",
                "aether mcp demo echo > empty.json && ",
                "test \"$(jq -r .echoed result.json)\" = redirect && ",
                "value=\"$(aether mcp demo echo --json '{\"message\":\"substitution\"}' | jq -r .echoed)\" && ",
                "test \"$value\" = substitution && ./call.sh"
            )
            .into(),
            ..Default::default()
        },
        Some(workspace.path()),
        &environment,
    )
    .await
    .unwrap();

    assert_eq!(output.exit_code, 0, "output: {}", output.output);
    assert_eq!(output.output.trim(), "script");
    assert_eq!(std::fs::read_to_string(workspace.path().join("pipe.txt")).unwrap().trim(), "pipe");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(
            &std::fs::read_to_string(workspace.path().join("result.json")).unwrap()
        )
        .unwrap(),
        json!({"echoed": "redirect"})
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(
            &std::fs::read_to_string(workspace.path().join("empty.json")).unwrap()
        )
        .unwrap(),
        json!({"echoed": null})
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn usage_errors_exit_two_and_runtime_errors_exit_one() {
    let (_server, socket) = gateway();

    for args in [
        vec!["mcp", "demo"],
        vec!["mcp", "demo", "echo", "--json"],
        vec!["mcp", "demo", "echo", "--json", "[]"],
        vec!["mcp", "demo", "echo", "--json", "1"],
        vec!["mcp", "demo", "echo", "--json", "not-json"],
        vec!["mcp", "demo", "echo", "--json", "{}", "--json", "{}"],
        vec!["mcp", "demo", "echo", "--timeout", "0"],
        vec!["mcp", "demo", "echo", "--unknown"],
        vec!["mcp", "missing", "--help"],
        vec!["mcp", "demo", "missing", "--help"],
    ] {
        let output = run(&socket, &args, None);
        assert_eq!(output.status.code(), Some(2), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    }

    let explicit_json_wins =
        run(&socket, &["mcp", "demo", "echo", "--json", r#"{"message":"flag"}"#], Some(r#"{"message":"stdin"}"#));
    assert_success(&explicit_json_wins);
    assert_eq!(stdout_json(&explicit_json_wins), json!({"echoed": "flag"}));

    let tool_error = run(&socket, &["mcp", "demo", "fail"], None);
    assert_eq!(tool_error.status.code(), Some(1), "stderr: {}", String::from_utf8_lossy(&tool_error.stderr));

    let timeout = run(&socket, &["mcp", "demo", "slow", "--timeout", "1"], None);
    assert_eq!(timeout.status.code(), Some(1), "stderr: {}", String::from_utf8_lossy(&timeout.stderr));
    assert!(String::from_utf8_lossy(&timeout.stderr).contains("timed out"));

    let missing = Command::new(env!("CARGO_BIN_EXE_aether"))
        .args(["mcp", "--help"])
        .env_remove(AETHER_MCP_IPC_SOCKET)
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&missing.stderr).contains(AETHER_MCP_IPC_SOCKET));
}

fn gateway() -> (mcp_utils::tool_gateway::UnixSocketServer, String) {
    let path = UnixSocketPath::new().unwrap();
    let transport = UnixSocketMcpTransport::bind(path).unwrap();
    let socket = transport.path().to_string_lossy().into_owned();
    (transport.spawn(FakeGateway), socket)
}

fn run(socket: &str, args: &[&str], stdin: Option<&str>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_aether"));
    command.args(args).env(AETHER_MCP_IPC_SOCKET, socket);
    if let Some(input) = stdin {
        use std::io::Write;
        let mut child = command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
        child.wait_with_output().unwrap()
    } else {
        command.stdin(Stdio::null()).output().unwrap()
    }
}

fn assert_success(output: &Output) {
    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
}

fn assert_stdout_contains(output: &Output, expected: &str) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(expected), "stdout did not contain {expected:?}: {stdout}");
}

fn stdout_json(output: &Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout).unwrap()
}
