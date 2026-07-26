//! End-to-end tests for LSP diagnostics through the MCP tool layer (TypeScript).
//!
//! These tests verify the full pipeline:
//!   file edits → LSP daemon → typescript-language-server diagnostics → queryable via `lsp_check_errors`
//!
//! Requirements:
//! - `npm` must be installed (the test project installs pinned TypeScript tooling locally)
//! - `aether-lspd` binary must be built (`cargo build -p aether-lspd`)
//!
//! Run with: `cargo test -p mcp-servers -- --ignored lsp_ts_diagnostics`

mod common;

use aether_lspd::testing::{NodeProject, TestProject};
use common::{call_tool, connect_lsp, has_errors, has_no_errors, poll_diagnostics};

/// Test: MCP `edit_file` tool → `typescript-language-server` picks up change → diagnostics queryable
#[tokio::test]
async fn test_ts_mcp_edit_produces_diagnostics() {
    let project = NodeProject::new("ts_mcp_edit_diag").expect("Failed to create project");
    project
        .add_file("src/index.ts", "const x: number = \"not a number\";\nconsole.log(x);\n")
        .expect("Failed to add file");

    let index_ts = project.file_path_str("src/index.ts");

    let (_server_handle, client) = connect_lsp(&project).await;

    let result = poll_diagnostics(&client, Some(&index_ts), has_errors).await;
    let errors = result["summary"]["errors"].as_u64().unwrap();
    assert!(errors > 0, "Expected type error diagnostics");

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

    poll_diagnostics(&client, Some(&index_ts), has_no_errors).await;

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

    let result = poll_diagnostics(&client, Some(&index_ts), has_errors).await;
    let errors = result["summary"]["errors"].as_u64().unwrap();
    assert!(errors > 0, "Expected type error after re-introducing bug");
}

/// Test: External `fs::write` → file watcher → diagnostics queryable (TypeScript)
#[tokio::test]
async fn test_ts_external_file_change_produces_diagnostics() {
    let project = NodeProject::new("ts_ext_write_diag").expect("Failed to create project");
    project
        .add_file("src/index.ts", "const x: number = \"not a number\";\nconsole.log(x);\n")
        .expect("Failed to add file");

    let index_ts = project.file_path_str("src/index.ts");
    let index_ts_path = project.root().join("src/index.ts");

    let (_server_handle, client) = connect_lsp(&project).await;

    let result = poll_diagnostics(&client, Some(&index_ts), has_errors).await;
    let errors = result["summary"]["errors"].as_u64().unwrap();
    assert!(errors > 0, "Expected type error diagnostics");

    std::fs::write(&index_ts_path, "const x: number = 42;\nconsole.log(x);\n").expect("Failed to write file");

    poll_diagnostics(&client, Some(&index_ts), has_no_errors).await;

    std::fs::write(&index_ts_path, "const x: number = true;\nconsole.log(x);\n").expect("Failed to write file");

    let result = poll_diagnostics(&client, Some(&index_ts), has_errors).await;
    let errors = result["summary"]["errors"].as_u64().unwrap();
    assert!(errors > 0, "Expected type error after external write");
}

/// Regression test: after `edit_file`, a SINGLE `lsp_check_errors` call (no polling)
/// should return fresh diagnostics for TypeScript files.
#[tokio::test]
async fn test_ts_diagnostics_after_edit_without_polling() {
    let project = NodeProject::new("ts_diag_no_poll").expect("Failed to create project");
    project.add_file("src/index.ts", "const x: number = 42;\nconsole.log(x);\n").expect("Failed to add file");

    let index_ts = project.file_path_str("src/index.ts");

    let (_server_handle, client) = connect_lsp(&project).await;

    poll_diagnostics(&client, Some(&index_ts), has_no_errors).await;

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

    // tsserver is slower than rust-analyzer, so the daemon needs extra time to publish fresh
    // diagnostics before the single (non-polling) call below.
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    let result = call_tool(&client, "lsp_check_errors", serde_json::json!({ "filePath": index_ts })).await;

    let errors = result["summary"]["errors"].as_u64().unwrap_or(0);
    assert!(
        errors > 0,
        "Expected diagnostics after edit + single lsp_check_errors call, got 0 errors. \
         Full result: {result}"
    );
}
