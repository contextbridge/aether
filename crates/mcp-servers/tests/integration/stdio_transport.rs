#![cfg(feature = "subagents")]

use crate::common::{McpClientExt, TestResult};
use mcp_utils::client::{ClientError, ClientOptions, McpClient, Transport};
use serde_json::json;
use std::collections::HashMap;

#[tokio::test]
async fn builtin_servers_list_their_tools_over_stdio() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let dir = dir.path().to_str().unwrap();
    let coding = &[
        "bash",
        "edit_file",
        "lsp_check_errors",
        "lsp_rename",
        "lsp_symbol",
        "read_file",
        "web_fetch",
        "web_search",
        "write_file",
    ];
    let cases: [(&str, &[&str], &[&str]); 6] = [
        ("tasks", &["--dir", dir], &["task_create", "task_get", "task_list", "task_update"]),
        ("coding", &[], coding),
        ("coding", &["--rules-dir", dir], coding),
        ("skills", &["--dir", dir], &["get_skills", "list_skills"]),
        ("subagents", &["--dir", dir], &["spawn_subagent"]),
        ("review", &[], &["review_artifact"]),
    ];

    for (server, args, expected) in cases {
        assert_eq!(list_tools(server, args).await, expected, "{server} {args:?}");
    }
}

#[tokio::test]
async fn tasks_server_create_and_get_task_over_stdio() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().to_str().unwrap();
    let client = connect(&["--server", "tasks", "--", "--dir", dir], HashMap::new()).await?;

    let created =
        client.call("task_create", json!({ "title": "Test task", "description": "Created over stdio" })).await?;
    let fetched = client.call("task_get", json!({ "id": created["task"]["id"] })).await?;

    assert_eq!(fetched["task"]["title"], "Test task");
    Ok(())
}

#[tokio::test]
async fn unknown_servers_exit_with_error() {
    for server in ["lsp", "nonexistent"] {
        assert!(connect(&["--server", server], HashMap::new()).await.is_err(), "{server} should be unavailable");
    }
}

async fn connect(args: &[&str], env: HashMap<String, String>) -> Result<McpClient, ClientError> {
    let command = env!("CARGO_BIN_EXE_mcp-servers-stdio").to_string();
    let args = args.iter().map(ToString::to_string).collect();
    McpClient::connect("stdio", Transport::Stdio { command, args, env, cwd: None }, &ClientOptions::default()).await
}

async fn list_tools(server: &str, server_args: &[&str]) -> Vec<String> {
    let aether_home = tempfile::tempdir().expect("create temp aether home");
    let env = HashMap::from([("AETHER_HOME".to_string(), aether_home.path().join(".aether").display().to_string())]);
    let args = [&["--server", server, "--"], server_args].concat();
    let client = connect(&args, env).await.expect("connect to server");
    let mut names: Vec<_> =
        client.list_tools().await.expect("list tools").into_iter().map(|t| t.name.to_string()).collect();
    names.sort();
    names
}
