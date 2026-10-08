#![cfg(feature = "stdio")]

use mcp_utils::McpError;
use mcp_utils::client::{ClientOptions, McpClient, ToolCallOptions, Transport};
use std::collections::HashMap;

async fn connect(args: &[&str], env: HashMap<String, String>) -> Result<McpClient, McpError> {
    let command = env!("CARGO_BIN_EXE_mcp-servers-stdio").to_string();
    let args = args.iter().map(ToString::to_string).collect();
    McpClient::connect("stdio", Transport::Stdio { command, args, env }, &ClientOptions::default()).await
}

fn tool_names(tools: &[rmcp::model::Tool]) -> Vec<&str> {
    tools.iter().map(|t| t.name.as_ref()).collect()
}

fn extract_text(content: &rmcp::model::ContentBlock) -> &str {
    content.as_text().expect("expected text content").text.as_str()
}

async fn connect_and_list_tools(server: &str, extra_args: &[&str]) -> Vec<rmcp::model::Tool> {
    let aether_home = tempfile::tempdir().expect("create temp aether home");
    let env = HashMap::from([("AETHER_HOME".to_string(), aether_home.path().join(".aether").display().to_string())]);
    let args = [&["--server", server], extra_args].concat();
    let client = connect(&args, env).await.expect("connect to server");
    client.list_tools().await.expect("list tools")
}

#[tokio::test]
async fn tasks_server_lists_tools_over_stdio() {
    let tmp = tempfile::tempdir().expect("create temp dir");

    let tools = connect_and_list_tools("tasks", &["--", "--dir", tmp.path().to_str().unwrap()]).await;
    let names = tool_names(&tools);

    assert!(names.contains(&"task_create"), "expected task_create, got: {names:?}");
    assert!(names.contains(&"task_list"), "expected task_list, got: {names:?}");
    assert!(names.contains(&"task_update"), "expected task_update, got: {names:?}");
    assert!(names.contains(&"task_get"), "expected task_get, got: {names:?}");
}

#[tokio::test]
async fn tasks_server_create_and_get_task_over_stdio() {
    let tmp = tempfile::tempdir().expect("create temp dir");

    let dir = tmp.path().to_str().unwrap();
    let client = connect(&["--server", "tasks", "--", "--dir", dir], HashMap::new()).await.expect("connect to server");

    let create_result = client
        .call_tool(
            "task_create",
            serde_json::json!({
                "title": "Test task",
                "description": "A test task created over stdio"
            })
            .as_object()
            .unwrap()
            .clone(),
            ToolCallOptions::default(),
        )
        .result()
        .await
        .expect("call task_create");

    let text = extract_text(create_result.content.first().expect("response has content"));
    let created: serde_json::Value = serde_json::from_str(text).expect("parse JSON response");
    let task_id = created["task"]["id"].as_str().expect("task has id");

    let get_result = client
        .call_tool(
            "task_get",
            serde_json::json!({ "id": task_id }).as_object().unwrap().clone(),
            ToolCallOptions::default(),
        )
        .result()
        .await
        .expect("call task_get");

    let get_text = extract_text(get_result.content.first().expect("response has content"));
    let fetched: serde_json::Value = serde_json::from_str(get_text).expect("parse JSON response");
    assert_eq!(fetched["task"]["title"].as_str(), Some("Test task"));
}

#[tokio::test]
async fn coding_server_lists_tools_over_stdio() {
    let tools = connect_and_list_tools("coding", &[]).await;
    let names = tool_names(&tools);

    assert!(names.contains(&"bash"), "expected bash tool, got: {names:?}");
    for removed in ["find", "grep", "ast_grep", "list_files", "lsp_document", "lsp_workspace_search"] {
        assert!(!names.contains(&removed), "unexpected removed tool {removed}, got: {names:?}");
    }
    assert!(names.contains(&"read_file"), "expected read_file tool, got: {names:?}");

    // LSP tools should be in the coding server too
    assert!(names.contains(&"lsp_symbol"), "expected lsp_symbol in coding server, got: {names:?}");
    assert!(names.contains(&"lsp_check_errors"), "expected lsp_check_errors in coding server, got: {names:?}");
    assert!(names.contains(&"lsp_rename"), "expected lsp_rename in coding server, got: {names:?}");
}

#[tokio::test]
async fn coding_server_accepts_rules_dir_over_stdio() {
    let tmp = tempfile::tempdir().expect("create temp dir");

    let tools = connect_and_list_tools("coding", &["--", "--rules-dir", tmp.path().to_str().unwrap()]).await;
    let names = tool_names(&tools);

    assert!(names.contains(&"bash"), "expected bash tool, got: {names:?}");
    for removed in ["find", "grep", "ast_grep", "list_files"] {
        assert!(!names.contains(&removed), "unexpected removed tool {removed}, got: {names:?}");
    }
    assert!(names.contains(&"read_file"), "expected read_file tool, got: {names:?}");
}

#[tokio::test]
async fn lsp_server_is_not_available_over_stdio() {
    let result = connect(&["--server", "lsp"], HashMap::new()).await;

    assert!(result.is_err(), "expected lsp server to be unavailable");
}

#[tokio::test]
async fn skills_server_lists_tools_over_stdio() {
    let tmp = tempfile::tempdir().expect("create temp dir");

    let tools = connect_and_list_tools("skills", &["--", "--dir", tmp.path().to_str().unwrap()]).await;
    let names = tool_names(&tools);

    assert!(names.contains(&"list_skills"), "expected list_skills, got: {names:?}");
    assert!(names.contains(&"get_skills"), "expected get_skills, got: {names:?}");
}

#[tokio::test]
async fn skills_server_accepts_deprecated_notes_dir_over_stdio() {
    let tmp = tempfile::tempdir().expect("create temp dir");

    let tools = connect_and_list_tools(
        "skills",
        &["--", "--dir", tmp.path().to_str().unwrap(), "--notes-dir", tmp.path().to_str().unwrap()],
    )
    .await;
    let names = tool_names(&tools);

    assert!(names.contains(&"list_skills"), "expected list_skills, got: {names:?}");
    assert!(names.contains(&"get_skills"), "expected get_skills, got: {names:?}");
}

#[tokio::test]
async fn subagents_server_lists_tools_over_stdio() {
    let tmp = tempfile::tempdir().expect("create temp dir");

    let tools = connect_and_list_tools("subagents", &["--", "--dir", tmp.path().to_str().unwrap()]).await;
    let names = tool_names(&tools);

    assert!(names.contains(&"spawn_subagent"), "expected spawn_subagent, got: {names:?}");
    assert_eq!(names.len(), 1, "expected exactly 1 tool, got: {names:?}");
}

#[tokio::test]
async fn review_server_lists_tools_over_stdio() {
    let tools = connect_and_list_tools("review", &[]).await;
    let names = tool_names(&tools);

    assert!(names.contains(&"review_artifact"), "expected review_artifact, got: {names:?}");
    assert_eq!(names.len(), 1, "expected exactly 1 tool, got: {names:?}");
}

#[tokio::test]
async fn unknown_server_exits_with_error() {
    let result = connect(&["--server", "nonexistent"], HashMap::new()).await;

    assert!(result.is_err(), "expected error for unknown server name");
}
