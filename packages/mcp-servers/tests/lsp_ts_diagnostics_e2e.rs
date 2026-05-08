//! End-to-end tests for LSP diagnostics through the MCP tool layer (TypeScript).
//!
//! These tests verify the full pipeline:
//!   file edits → LSP daemon → typescript-language-server diagnostics → queryable via `lsp_check_errors`
//!
//! Requirements:
//! - `typescript-language-server` must be installed and in PATH
//! - `typescript` must be installed (globally or in PATH)
//! - `aether-lspd` binary must be built (`cargo build -p aether-lspd`)
//!
//! Run with: `cargo test -p mcp-servers -- --ignored lsp_ts_diagnostics`

mod common;

use aether_lspd::testing::{NodeProject, TestProject};
use common::{call_tool, connect_lsp, has_errors, has_no_errors, poll_diagnostics};
use rmcp::RoleClient;
use rmcp::model::ClientInfo;
use rmcp::service::RunningService;

async fn set_line(
    client: &RunningService<RoleClient, ClientInfo>,
    file_path: &str,
    line_number: usize,
    new_text: &str,
) {
    call_tool(client, "read_file", serde_json::json!({ "filePath": file_path })).await;
    call_tool(
        client,
        "edit_file",
        serde_json::json!({
            "filePath": file_path,
            "edits": [{"type": "set_line", "line": line_number, "newText": new_text}]
        }),
    )
    .await;
}

/// Test: MCP `edit_file` tool → `typescript-language-server` picks up change → diagnostics queryable
#[tokio::test]
async fn test_ts_mcp_edit_produces_diagnostics() {
    // 1. Create a Node project with a type error
    let project = NodeProject::new("ts_mcp_edit_diag").expect("Failed to create project");
    project
        .add_file("src/index.ts", "const x: number = \"not a number\";\nconsole.log(x);\n")
        .expect("Failed to add file");

    let index_ts = project.file_path_str("src/index.ts");

    // 2. Start CodingMcp with LSP enabled
    let (_server_handle, client) = connect_lsp(&project).await;

    // 3. Wait for typescript-language-server to report the initial type error
    let result = poll_diagnostics(&client, Some(&index_ts), has_errors).await;
    let errors = result["summary"]["errors"].as_u64().unwrap();
    assert!(errors > 0, "Expected type error diagnostics");

    // 4. Fix the error using MCP tools: read_file then edit_file
    set_line(&client, &index_ts, 1, "const x: number = 42;").await;

    // 5. Poll until errors clear
    poll_diagnostics(&client, Some(&index_ts), has_no_errors).await;

    // 6. Re-introduce a different error via MCP edit
    set_line(&client, &index_ts, 1, "const x: number = true;").await;

    // 7. Poll until errors reappear
    let result = poll_diagnostics(&client, Some(&index_ts), has_errors).await;
    let errors = result["summary"]["errors"].as_u64().unwrap();
    assert!(errors > 0, "Expected type error after re-introducing bug");
}

/// Test: External `fs::write` → file watcher → diagnostics queryable (TypeScript)
#[tokio::test]
async fn test_ts_external_file_change_produces_diagnostics() {
    // 1. Create a Node project with a type error
    let project = NodeProject::new("ts_ext_write_diag").expect("Failed to create project");
    project
        .add_file("src/index.ts", "const x: number = \"not a number\";\nconsole.log(x);\n")
        .expect("Failed to add file");

    let index_ts = project.file_path_str("src/index.ts");
    let index_ts_path = project.root().join("src/index.ts");

    // 2. Start CodingMcp with LSP enabled
    let (_server_handle, client) = connect_lsp(&project).await;

    // 3. Wait for typescript-language-server to report the initial type error
    let result = poll_diagnostics(&client, Some(&index_ts), has_errors).await;
    let errors = result["summary"]["errors"].as_u64().unwrap();
    assert!(errors > 0, "Expected type error diagnostics");

    // 4. Fix the error via direct filesystem write (bypassing MCP tools)
    std::fs::write(&index_ts_path, "const x: number = 42;\nconsole.log(x);\n").expect("Failed to write file");

    // 5. Poll until errors clear (file watcher → didChangeWatchedFiles → tsserver re-reads)
    poll_diagnostics(&client, Some(&index_ts), has_no_errors).await;

    // 6. Introduce a new error via direct filesystem write
    std::fs::write(&index_ts_path, "const x: number = true;\nconsole.log(x);\n").expect("Failed to write file");

    // 7. Poll until errors reappear
    let result = poll_diagnostics(&client, Some(&index_ts), has_errors).await;
    let errors = result["summary"]["errors"].as_u64().unwrap();
    assert!(errors > 0, "Expected type error after external write");
}

/// Regression test: after `edit_file`, a SINGLE `lsp_check_errors` call (no polling)
/// should return fresh diagnostics for TypeScript files.
#[tokio::test]
async fn test_ts_diagnostics_after_edit_without_polling() {
    // 1. Create a Node project with valid code
    let project = NodeProject::new("ts_diag_no_poll").expect("Failed to create project");
    project.add_file("src/index.ts", "const x: number = 42;\nconsole.log(x);\n").expect("Failed to add file");

    let index_ts = project.file_path_str("src/index.ts");

    // 2. Start CodingMcp with LSP enabled
    let (_server_handle, client) = connect_lsp(&project).await;

    // 3. Wait for initial indexing — no errors expected
    poll_diagnostics(&client, Some(&index_ts), has_no_errors).await;

    // 4. Introduce a type error via edit_file
    set_line(&client, &index_ts, 1, "const x: number = \"not a number\";").await;

    // 5. Wait for typescript-language-server to process, then make a SINGLE call.
    //    tsserver is slower than rust-analyzer, so give it more time.
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    let result = call_tool(
        &client,
        "lsp_check_errors",
        serde_json::json!({ "input": { "scope": "file", "filePath": index_ts } }),
    )
    .await;

    let errors = result["summary"]["errors"].as_u64().unwrap_or(0);
    assert!(
        errors > 0,
        "Expected diagnostics after edit + single lsp_check_errors call, got 0 errors. \
         Full result: {result}"
    );
}
