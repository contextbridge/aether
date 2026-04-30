#![cfg(feature = "stdio")]

use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use rmcp::transport::TokioChildProcess;
use std::process::Stdio;
use tokio::process::Command;

fn stdio_binary() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mcp-servers-stdio"));
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped());
    cmd
}

fn tool_names(tools: &[rmcp::model::Tool]) -> Vec<&str> {
    tools.iter().map(|t| t.name.as_ref()).collect()
}

fn extract_text(content: &rmcp::model::Content) -> &str {
    content.as_text().expect("expected text content").text.as_str()
}

async fn connect_and_list_tools(server: &str, extra_args: &[&str]) -> Vec<rmcp::model::Tool> {
    let aether_home = tempfile::tempdir().expect("create temp aether home");
    let mut cmd = stdio_binary();
    cmd.env("AETHER_HOME", aether_home.path().join(".aether"));
    cmd.arg("--server").arg(server);
    for arg in extra_args {
        cmd.arg(arg);
    }
    let transport = TokioChildProcess::new(cmd).expect("spawn stdio server");
    let client = ().serve(transport).await.expect("connect to server");
    client.peer().list_all_tools().await.expect("list tools")
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

    let mut cmd = stdio_binary();
    cmd.arg("--server").arg("tasks");
    cmd.arg("--").arg("--dir").arg(tmp.path());

    let transport = TokioChildProcess::new(cmd).expect("spawn stdio server");
    let client = ().serve(transport).await.expect("connect to server");

    // This test exercises call_tool, so it doesn't use connect_and_list_tools.
    // Create a task
    let create_result = client
        .peer()
        .call_tool(
            rmcp::model::CallToolRequestParams::new("task_create").with_arguments(
                serde_json::json!({
                    "title": "Test task",
                    "description": "A test task created over stdio"
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .await
        .expect("call task_create");

    let text = extract_text(create_result.content.first().expect("response has content"));
    let created: serde_json::Value = serde_json::from_str(text).expect("parse JSON response");
    let task_id = created["task"]["id"].as_str().expect("task has id");

    // Get the task back
    let get_result = client
        .peer()
        .call_tool(
            CallToolRequestParams::new("task_get")
                .with_arguments(serde_json::json!({ "id": task_id }).as_object().unwrap().clone()),
        )
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

    assert!(names.contains(&"grep"), "expected grep tool, got: {names:?}");
    assert!(names.contains(&"read_file"), "expected read_file tool, got: {names:?}");

    // LSP tools should be in the coding server too
    assert!(names.contains(&"lsp_symbol"), "expected lsp_symbol in coding server, got: {names:?}");
    assert!(names.contains(&"lsp_document"), "expected lsp_document in coding server, got: {names:?}");
    assert!(names.contains(&"lsp_check_errors"), "expected lsp_check_errors in coding server, got: {names:?}");
}

#[tokio::test]
async fn coding_server_accepts_rules_dir_over_stdio() {
    let tmp = tempfile::tempdir().expect("create temp dir");

    let tools = connect_and_list_tools("coding", &["--", "--rules-dir", tmp.path().to_str().unwrap()]).await;
    let names = tool_names(&tools);

    assert!(names.contains(&"grep"), "expected grep tool, got: {names:?}");
    assert!(names.contains(&"read_file"), "expected read_file tool, got: {names:?}");
}

#[tokio::test]
async fn lsp_server_lists_tools_over_stdio() {
    let tmp = tempfile::tempdir().expect("create temp dir");

    let tools = connect_and_list_tools("lsp", &["--", "--root-dir", tmp.path().to_str().unwrap()]).await;
    let names = tool_names(&tools);

    assert!(names.contains(&"lsp_symbol"), "expected lsp_symbol, got: {names:?}");
    assert!(names.contains(&"lsp_document"), "expected lsp_document, got: {names:?}");
    assert!(names.contains(&"lsp_check_errors"), "expected lsp_check_errors, got: {names:?}");
    assert!(names.contains(&"lsp_workspace_search"), "expected lsp_workspace_search, got: {names:?}");
    assert!(names.contains(&"lsp_rename"), "expected lsp_rename, got: {names:?}");
    assert_eq!(names.len(), 5, "expected exactly 5 tools, got: {names:?}");
}

#[tokio::test]
async fn skills_server_lists_tools_over_stdio() {
    let tmp = tempfile::tempdir().expect("create temp dir");
    let notes_dir = tmp.path().join("notes");

    let tools = connect_and_list_tools(
        "skills",
        &["--", "--dir", tmp.path().to_str().unwrap(), "--notes-dir", notes_dir.to_str().unwrap()],
    )
    .await;
    let names = tool_names(&tools);

    assert!(names.contains(&"list_skills"), "expected list_skills, got: {names:?}");
    assert!(names.contains(&"get_skills"), "expected get_skills, got: {names:?}");
    assert!(names.contains(&"save_note"), "expected save_note, got: {names:?}");
    assert!(names.contains(&"search_notes"), "expected search_notes, got: {names:?}");
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
async fn survey_server_lists_tools_over_stdio() {
    let tools = connect_and_list_tools("survey", &[]).await;
    let names = tool_names(&tools);

    assert!(names.contains(&"ask_user"), "expected ask_user, got: {names:?}");
    assert_eq!(names.len(), 1, "expected exactly 1 tool, got: {names:?}");
}

#[tokio::test]
async fn unknown_server_exits_with_error() {
    let mut cmd = stdio_binary();
    cmd.arg("--server").arg("nonexistent");

    let transport = TokioChildProcess::new(cmd).expect("spawn process");
    let result = ().serve(transport).await;

    assert!(result.is_err(), "expected error for unknown server name");
}
