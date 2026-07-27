//! End-to-end tests for LSP diagnostics through the MCP tool layer.
//!
//! These tests verify the full pipeline:
//!   file edits → LSP daemon → rust-analyzer diagnostics → queryable via `lsp_check_errors`
//!
//! Requirements:
//! - `rust-analyzer` must be installed and in PATH
//! - `aether-lspd` binary must be built (`cargo build -p aether-lspd`)
//!
//! Run with: `cargo test -p mcp-servers -- lsp_diagnostics`

use crate::common::{call_tool, connect_lsp, has_errors, has_no_errors, poll_diagnostics, try_call_tool};
use aether_lspd::testing::{CargoProject, TestProject};
use rmcp::RoleClient;
use rmcp::model::ClientInfo;
use rmcp::service::RunningService;
use std::path::PathBuf;
use std::time::{Duration, Instant};

async fn poll_workspace_diagnostics(
    client: &RunningService<RoleClient, ClientInfo>,
    predicate: impl Fn(&serde_json::Value) -> bool,
    timeout: Duration,
) -> serde_json::Value {
    let start = Instant::now();
    let mut last_result = None;

    while start.elapsed() < timeout {
        if let Some(result) = try_call_tool(client, "lsp_check_errors", serde_json::json!({})).await {
            if predicate(&result) {
                return result;
            }
            last_result = Some(result);
        }

        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    panic!(
        "workspace diagnostics timed out after {timeout:?}. Last result: {}",
        last_result.as_ref().map_or_else(|| "(no valid response)".to_string(), std::string::ToString::to_string)
    );
}

fn file_error_count(result: &serde_json::Value, file_path: &str) -> usize {
    let expected_path = canonical_path(file_path);
    result["diagnostics"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|diagnostic| {
            diagnostic["file"].as_str().map(canonical_path).as_deref() == Some(expected_path.as_str())
                && diagnostic["severity"].as_str() == Some("error")
        })
        .count()
}

fn canonical_path(path: &str) -> String {
    std::fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path)).to_string_lossy().to_string()
}

/// Test: MCP `edit_file` tool → rust-analyzer picks up change → diagnostics queryable
#[tokio::test]
async fn test_mcp_edit_produces_diagnostics() {
    let project = CargoProject::new("mcp_edit_diag").expect("Failed to create project");
    project
        .add_file(
            "src/main.rs",
            r#"fn main() {
    let x: i32 = "not an int";
    println!("{}", x);
}
"#,
        )
        .expect("Failed to add file");

    let main_rs = project.file_path_str("src/main.rs");

    let (_server_handle, client) = connect_lsp(&project).await;

    let result = poll_diagnostics(&client, Some(&main_rs), has_errors).await;
    let errors = result["summary"]["errors"].as_u64().unwrap();
    assert!(errors > 0, "Expected type error diagnostics");

    call_tool(&client, "read_file", serde_json::json!({ "filePath": main_rs })).await;

    call_tool(
        &client,
        "edit_file",
        serde_json::json!({
            "filePath": main_rs,
            "edits": [{ "oldString": "\"not an int\"", "newString": "42" }]
        }),
    )
    .await;

    poll_diagnostics(&client, Some(&main_rs), has_no_errors).await;

    call_tool(&client, "read_file", serde_json::json!({ "filePath": main_rs })).await;

    call_tool(
        &client,
        "edit_file",
        serde_json::json!({
            "filePath": main_rs,
            "edits": [{ "oldString": "42", "newString": "true" }]
        }),
    )
    .await;

    let result = poll_diagnostics(&client, Some(&main_rs), has_errors).await;
    let errors = result["summary"]["errors"].as_u64().unwrap();
    assert!(errors > 0, "Expected type error after re-introducing bug");
}

/// Regression test: after `edit_file`, a SINGLE `lsp_check_errors` call (no polling)
/// should eventually return fresh diagnostics. This verifies the daemon waits for
/// the LSP to re-publish diagnostics after syncing a changed document.
#[tokio::test]
async fn test_diagnostics_available_after_edit_without_polling() {
    let project = CargoProject::new("diag_after_edit_no_poll").expect("Failed to create project");
    project
        .add_file(
            "src/main.rs",
            r#"fn main() {
    let x: i32 = 42;
    println!("{}", x);
}
"#,
        )
        .expect("Failed to add file");

    let main_rs = project.file_path_str("src/main.rs");

    let (_server_handle, client) = connect_lsp(&project).await;

    poll_diagnostics(&client, Some(&main_rs), has_no_errors).await;

    call_tool(&client, "read_file", serde_json::json!({ "filePath": main_rs })).await;

    call_tool(
        &client,
        "edit_file",
        serde_json::json!({
            "filePath": main_rs,
            "edits": [{ "oldString": "42", "newString": "\"not an int\"" }]
        }),
    )
    .await;

    poll_diagnostics(&client, Some(&main_rs), has_errors).await;
}

/// Regression test: after `edit_file`, calling `lsp_check_errors` in workspace scope
/// should still return fresh diagnostics. This verifies the daemon syncs all open
/// documents before returning the cache.
#[tokio::test]
async fn test_diagnostics_all_files_after_edit() {
    let project = CargoProject::new("diag_all_files").expect("Failed to create project");
    project
        .add_file(
            "src/main.rs",
            r#"fn main() {
    let x: i32 = 42;
    println!("{}", x);
}
"#,
        )
        .expect("Failed to add file");

    let main_rs = project.file_path_str("src/main.rs");

    let (_server_handle, client) = connect_lsp(&project).await;

    poll_diagnostics(&client, Some(&main_rs), has_no_errors).await;

    call_tool(&client, "read_file", serde_json::json!({ "filePath": main_rs })).await;

    call_tool(
        &client,
        "edit_file",
        serde_json::json!({
            "filePath": main_rs,
            "edits": [{ "oldString": "42", "newString": "\"not an int\"" }]
        }),
    )
    .await;

    poll_diagnostics(&client, None, has_errors).await;
}

/// Regression test: after `edit_file`, an immediate workspace-scoped
/// `lsp_check_errors` call should report the new error even when no file-scoped
/// diagnostics request has ever been made.
#[tokio::test]
async fn test_workspace_diagnostics_after_edit_without_file_check() {
    let project = CargoProject::new("diag_workspace_after_edit").expect("Failed to create project");
    project
        .add_file(
            "src/main.rs",
            r#"fn main() {
    let x: i32 = 42;
    println!("{}", x);
}
"#,
        )
        .expect("Failed to add file");

    let main_rs = project.file_path_str("src/main.rs");

    let (_server_handle, client) = connect_lsp(&project).await;

    poll_diagnostics(&client, None, has_no_errors).await;

    call_tool(&client, "read_file", serde_json::json!({ "filePath": main_rs })).await;

    call_tool(
        &client,
        "edit_file",
        serde_json::json!({
            "filePath": main_rs,
            "edits": [{ "oldString": "42", "newString": "\"not an int\"" }]
        }),
    )
    .await;

    poll_workspace_diagnostics(&client, |result| file_error_count(result, &main_rs) > 0, Duration::from_secs(30)).await;
}

/// Regression test: once workspace diagnostics have recorded an error, fixing the
/// file via `edit_file` should immediately clear the workspace result without any
/// file-scoped diagnostics request.
#[tokio::test]
async fn test_workspace_diagnostics_clear_after_fix_without_file_check() {
    let project = CargoProject::new("diag_workspace_fix").expect("Failed to create project");
    project
        .add_file(
            "src/main.rs",
            r#"fn main() {
    let x: i32 = "not an int";
    println!("{}", x);
}
"#,
        )
        .expect("Failed to add file");

    let main_rs = project.file_path_str("src/main.rs");

    let (_server_handle, client) = connect_lsp(&project).await;

    let initial =
        poll_workspace_diagnostics(&client, |result| file_error_count(result, &main_rs) > 0, Duration::from_mins(1))
            .await;
    assert!(
        file_error_count(&initial, &main_rs) > 0,
        "Expected bootstrap workspace diagnostics to report the initial error. Full result: {initial}"
    );

    call_tool(&client, "read_file", serde_json::json!({ "filePath": main_rs })).await;

    call_tool(
        &client,
        "edit_file",
        serde_json::json!({
            "filePath": main_rs,
            "edits": [{ "oldString": "\"not an int\"", "newString": "42" }]
        }),
    )
    .await;

    let result =
        poll_workspace_diagnostics(&client, |r| file_error_count(r, &main_rs) == 0, Duration::from_mins(1)).await;
    assert_eq!(
        file_error_count(&result, &main_rs),
        0,
        "Expected workspace diagnostics to clear after fixing the file. \
         Full result: {result}"
    );
}

/// Regression test: after an EXTERNAL file edit (e.g. user's editor), calling
/// `lsp_check_errors` in workspace scope should detect the change and return
/// fresh diagnostics. This verifies the daemon syncs files from the diagnostics
/// cache, not just previously-opened documents.
#[tokio::test]
async fn test_diagnostics_all_files_after_external_edit() {
    let project = CargoProject::new("diag_all_ext_edit").expect("Failed to create project");
    project
        .add_file(
            "src/main.rs",
            r#"fn main() {
    let x: i32 = 42;
    println!("{}", x);
}
"#,
        )
        .expect("Failed to add file");

    let main_rs = project.file_path_str("src/main.rs");
    let main_rs_path = project.root().join("src/main.rs");

    let (_server_handle, client) = connect_lsp(&project).await;

    poll_diagnostics(&client, Some(&main_rs), has_no_errors).await;

    std::fs::write(
        &main_rs_path,
        r#"fn main() {
    let x: i32 = "not an int";
    println!("{}", x);
}
"#,
    )
    .expect("Failed to write file");

    poll_diagnostics(&client, None, has_errors).await;
}

/// Regression test: after an EXTERNAL file edit, a SINGLE workspace-scoped
/// `lsp_check_errors` call (no polling) should return errors. The file watcher keeps the
/// diagnostics cache fresh, so the daemon should simply return whatever is cached.
#[tokio::test]
async fn test_diagnostics_all_files_after_external_edit_single_call() {
    let project = CargoProject::new("diag_ext_single_call").expect("Failed to create project");
    project
        .add_file(
            "src/main.rs",
            r#"fn main() {
    let x: i32 = 42;
    println!("{}", x);
}
"#,
        )
        .expect("Failed to add file");

    let main_rs = project.file_path_str("src/main.rs");
    let main_rs_path = project.root().join("src/main.rs");

    let (_server_handle, client) = connect_lsp(&project).await;

    poll_diagnostics(&client, Some(&main_rs), has_no_errors).await;

    std::fs::write(
        &main_rs_path,
        r#"fn main() {
    let x: i32 = "not an int";
    println!("{}", x);
}
"#,
    )
    .expect("Failed to write file");

    poll_diagnostics(&client, None, has_errors).await;
}

/// Test: External `fs::write` → file watcher → diagnostics queryable
#[tokio::test]
async fn test_external_file_change_produces_diagnostics() {
    let project = CargoProject::new("ext_write_diag").expect("Failed to create project");
    project
        .add_file(
            "src/main.rs",
            r#"fn main() {
    let x: i32 = "not an int";
    println!("{}", x);
}
"#,
        )
        .expect("Failed to add file");

    let main_rs = project.file_path_str("src/main.rs");
    let main_rs_path = project.root().join("src/main.rs");

    let (_server_handle, client) = connect_lsp(&project).await;

    let result = poll_diagnostics(&client, Some(&main_rs), has_errors).await;
    let errors = result["summary"]["errors"].as_u64().unwrap();
    assert!(errors > 0, "Expected type error diagnostics");

    std::fs::write(
        &main_rs_path,
        r#"fn main() {
    let x: i32 = 42;
    println!("{}", x);
}
"#,
    )
    .expect("Failed to write file");

    poll_diagnostics(&client, Some(&main_rs), has_no_errors).await;

    std::fs::write(
        &main_rs_path,
        r#"fn main() {
    let x: i32 = true;
    println!("{}", x);
}
"#,
    )
    .expect("Failed to write file");

    let result = poll_diagnostics(&client, Some(&main_rs), has_errors).await;
    let errors = result["summary"]["errors"].as_u64().unwrap();
    assert!(errors > 0, "Expected type error after external write");
}

/// Regression test: files discovered ONLY via the file watcher (never opened or
/// present in the diagnostics cache) should still appear in workspace scope.
///
/// Unlike every other test in this file, this test does NOT prime the diagnostics
/// cache by calling `poll_diagnostics` with a file path first. Instead, it polls
/// workspace-scope diagnostics (which does NOT open documents) until initial
/// indexing completes, then edits the file externally so the file watcher fires
/// `didChangeWatchedFiles`. If the daemon only consults `diagnostics_cache.keys()`
/// for workspace scope, this file will be invisible.
#[tokio::test]
async fn test_diagnostics_all_files_discovers_file_watcher_uris() {
    let project = CargoProject::new("diag_fw_discover").expect("Failed to create project");
    project
        .add_file(
            "src/main.rs",
            r#"fn main() {
    let x: i32 = 42;
    println!("{}", x);
}
"#,
        )
        .expect("Failed to add file");

    let main_rs_path = project.root().join("src/main.rs");

    let (_server_handle, client) = connect_lsp(&project).await;

    poll_diagnostics(&client, None, has_no_errors).await;

    std::fs::write(
        &main_rs_path,
        r#"fn main() {
    let x: i32 = "not an int";
    println!("{}", x);
}
"#,
    )
    .expect("Failed to write file");

    poll_diagnostics(&client, None, has_errors).await;
}
