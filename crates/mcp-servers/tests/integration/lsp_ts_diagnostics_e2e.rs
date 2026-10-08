use crate::common::{call_tool, connect_lsp, has_errors, has_no_errors};
use aether_lspd::testing::{NodeProject, TestProject};
use mcp_utils::client::McpClient;

#[tokio::test]
async fn test_ts_mcp_edit_produces_diagnostics() {
    let project = NodeProject::new("ts_mcp_edit_diag").expect("Failed to create project");
    project
        .add_file("src/index.ts", "const x: number = \"not a number\";\nconsole.log(x);\n")
        .expect("Failed to add file");

    let index_ts = project.file_path_str("src/index.ts");

    let client = connect_lsp(&project).await;

    let result = check_errors(&client, &index_ts).await;
    assert!(has_errors(&result), "Expected type error diagnostics: {result}");

    call_tool(&client, "read_file", serde_json::json!({ "filePath": index_ts })).await;

    call_tool(
        &client,
        "edit_file",
        serde_json::json!({
            "filePath": index_ts,
            "edits": [{ "oldString": "\"not a number\"", "newString": "42" }]
        }),
    )
    .await;

    let result = check_errors(&client, &index_ts).await;
    assert!(has_no_errors(&result), "Expected no errors after fixing the bug: {result}");

    call_tool(&client, "read_file", serde_json::json!({ "filePath": index_ts })).await;

    call_tool(
        &client,
        "edit_file",
        serde_json::json!({
            "filePath": index_ts,
            "edits": [{ "oldString": "42", "newString": "true" }]
        }),
    )
    .await;

    let result = check_errors(&client, &index_ts).await;
    assert!(has_errors(&result), "Expected type error after re-introducing bug: {result}");
}

#[tokio::test]
async fn test_ts_external_file_change_produces_diagnostics() {
    let project = NodeProject::new("ts_ext_write_diag").expect("Failed to create project");
    project
        .add_file("src/index.ts", "const x: number = \"not a number\";\nconsole.log(x);\n")
        .expect("Failed to add file");

    let index_ts = project.file_path_str("src/index.ts");
    let index_ts_path = project.root().join("src/index.ts");

    let client = connect_lsp(&project).await;

    let result = check_errors(&client, &index_ts).await;
    assert!(has_errors(&result), "Expected type error diagnostics: {result}");

    std::fs::write(&index_ts_path, "const x: number = 42;\nconsole.log(x);\n").expect("Failed to write file");

    let result = check_errors(&client, &index_ts).await;
    assert!(has_no_errors(&result), "Expected no errors after external fix: {result}");

    std::fs::write(&index_ts_path, "const x: number = true;\nconsole.log(x);\n").expect("Failed to write file");

    let result = check_errors(&client, &index_ts).await;
    assert!(has_errors(&result), "Expected type error after external write: {result}");
}

#[tokio::test]
async fn test_ts_diagnostics_after_edit_without_polling() {
    let project = NodeProject::new("ts_diag_no_poll").expect("Failed to create project");
    project.add_file("src/index.ts", "const x: number = 42;\nconsole.log(x);\n").expect("Failed to add file");

    let index_ts = project.file_path_str("src/index.ts");

    let client = connect_lsp(&project).await;

    call_tool(&client, "read_file", serde_json::json!({ "filePath": index_ts })).await;

    call_tool(
        &client,
        "edit_file",
        serde_json::json!({
            "filePath": index_ts,
            "edits": [{ "oldString": "42", "newString": "\"not a number\"" }]
        }),
    )
    .await;

    let result = check_errors(&client, &index_ts).await;
    assert!(has_errors(&result), "Expected diagnostics after edit + single lsp_check_errors call: {result}");
}

async fn check_errors(client: &McpClient, file_path: &str) -> serde_json::Value {
    call_tool(client, "lsp_check_errors", serde_json::json!({ "filePath": file_path })).await
}
