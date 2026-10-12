use aether_core::mcp::{AETHER_MCP_IPC_SOCKET, McpCatalog, ToolExposure, mcp};
use aether_core::testing::ready_catalog;
use mcp_servers::coding::tools::bash::{BashEnvironment, BashInput, execute_command};
use mcp_utils::gateway::ServerSpec;
use mcp_utils::model::CallToolResult;
use mcp_utils::testing::{FakeMcpServer, FakeTool, FakeToolResponse};
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output, Stdio};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn layered_help_reveals_servers_tools_and_complete_schema() {
    let (_session, socket) = gateway().await;

    let top = run(&socket, &["mcp", "--help"], None);
    assert_success(&top);
    assert_stdout_contains(&top, "demo");
    assert_stdout_contains(&top, "A fake MCP server for testing");

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
async fn server_help_supports_namespace_delimiters_in_server_names() {
    let (_session, socket) = gateway().await;

    let server = run(&socket, &["mcp", "deep__demo", "--help"], None);
    assert_success(&server);
    assert_stdout_contains(&server, "  ping");
    assert_stdout_contains(&server, "Ping a nested server");
    assert!(!String::from_utf8_lossy(&server.stdout).contains("demo__ping"));

    let demo = run(&socket, &["mcp", "demo", "--help"], None);
    assert_success(&demo);
    assert!(!String::from_utf8_lossy(&demo.stdout).contains("ping"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn server_help_lists_only_the_named_servers_tools() {
    let (_session, socket) = gateway().await;

    let docs = run(&socket, &["mcp", "docs", "--help"], None);
    assert_success(&docs);
    assert_stdout_contains(&docs, "Search the docs");
    assert!(!String::from_utf8_lossy(&docs.stdout).contains("Search the v2 docs"));

    let v2 = run(&socket, &["mcp", "docs__v2", "--help"], None);
    assert_success(&v2);
    assert_stdout_contains(&v2, "Search the v2 docs");
    assert!(!String::from_utf8_lossy(&v2.stdout).contains("Search the docs"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn calls_accept_empty_json_flag_or_stdin_and_print_one_json_value() {
    let (_session, socket) = gateway().await;

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
    let (_session, socket) = gateway().await;
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
    let (_session, socket) = gateway().await;
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
    let (_session, socket) = gateway().await;

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

/// A session whose servers all defer their tools, as `aether mcp` reaches them through the session's socket.
async fn gateway() -> (McpCatalog, String) {
    let demo = FakeMcpServer::new()
        .with_tool(
            FakeTool::new("echo")
                .description("Echo a message")
                .input_schema(json!({
                    "type": "object",
                    "properties": { "message": { "type": "string", "description": "Text to echo" } },
                    "required": ["message"]
                }))
                .responds_with(|request| {
                    let message = request.arguments.as_ref().and_then(|args| args.get("message")).cloned();
                    CallToolResult::structured(json!({ "echoed": message })).into()
                }),
        )
        .with_tool(FakeTool::new("fail").responds(CallToolResult::error(vec![])))
        .with_tool(FakeTool::new("slow").responds(FakeToolResponse::pending()))
        .with_tool(FakeTool::new("text").responds(FakeToolResponse::text("plain")));
    let servers = [
        ("demo", demo),
        ("deep__demo", FakeMcpServer::new().with_tool(FakeTool::new("ping").description("Ping a nested server"))),
        ("docs", FakeMcpServer::new().with_tool(FakeTool::new("search").description("Search the docs"))),
        ("docs__v2", FakeMcpServer::new().with_tool(FakeTool::new("search").description("Search the v2 docs"))),
    ];
    let builder = servers.into_iter().fold(mcp("/tmp"), |builder, (name, server)| {
        builder.with_server(ServerSpec::in_process(name, server), ToolExposure::deferred_all())
    });
    let catalog = ready_catalog(builder).await;
    let socket = catalog.deferred_tools_socket().expect("deferred servers are served").to_string_lossy().into_owned();
    (catalog, socket)
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
