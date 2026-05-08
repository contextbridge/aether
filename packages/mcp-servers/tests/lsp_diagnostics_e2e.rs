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

mod common;

use aether_lspd::testing::{CargoProject, TestProject};
use common::{call_tool, connect_lsp, has_errors, has_no_errors, poll_diagnostics, try_call_tool};
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
        if let Some(result) =
            try_call_tool(client, "lsp_check_errors", serde_json::json!({"input": {"scope": "workspace"}})).await
        {
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

/// Test: MCP `edit_file` tool → rust-analyzer picks up change → diagnostics queryable
#[tokio::test]
async fn test_mcp_edit_produces_diagnostics() {
    // 1. Create a Cargo project with a type error
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

    // 2. Start CodingMcp with LSP enabled
    let (_server_handle, client) = connect_lsp(&project).await;

    // 3. Wait for rust-analyzer to index and report the initial type error
    let result = poll_diagnostics(&client, Some(&main_rs), has_errors).await;
    let errors = result["summary"]["errors"].as_u64().unwrap();
    assert!(errors > 0, "Expected type error diagnostics");

    // 4. Fix the error using MCP tools: read_file then edit_file
    set_line(&client, &main_rs, 2, "    let x: i32 = 42;").await;
    // 5. Poll until errors clear
    poll_diagnostics(&client, Some(&main_rs), has_no_errors).await;

    // 6. Re-introduce a different error via MCP edit
    set_line(&client, &main_rs, 2, "    let x: i32 = true;").await;

    // 7. Poll until errors reappear
    let result = poll_diagnostics(&client, Some(&main_rs), has_errors).await;
    let errors = result["summary"]["errors"].as_u64().unwrap();
    assert!(errors > 0, "Expected type error after re-introducing bug");
}

/// Regression test: after `edit_file`, a SINGLE `lsp_check_errors` call (no polling)
/// should eventually return fresh diagnostics. This verifies the daemon waits for
/// the LSP to re-publish diagnostics after syncing a changed document.
#[tokio::test]
async fn test_diagnostics_available_after_edit_without_polling() {
    // 1. Create a Cargo project with valid code
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

    // 2. Start CodingMcp with LSP enabled
    let (_server_handle, client) = connect_lsp(&project).await;

    // 3. Wait for initial indexing — no errors expected
    poll_diagnostics(&client, Some(&main_rs), has_no_errors).await;

    // 4. Introduce a syntax error via edit_file
    set_line(&client, &main_rs, 2, "    let x: i32 = \"not an int\";").await;

    // 5. Poll until rust-analyzer reports errors for the edited file.
    poll_diagnostics(&client, Some(&main_rs), has_errors).await;
}

/// Regression test: after `edit_file`, calling `lsp_check_errors` in workspace scope
/// should still return fresh diagnostics. This verifies the daemon syncs all open
/// documents before returning the cache.
#[tokio::test]
async fn test_diagnostics_all_files_after_edit() {
    // 1. Create a Cargo project with valid code
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

    // 2. Start CodingMcp with LSP enabled
    let (_server_handle, client) = connect_lsp(&project).await;

    // 3. Wait for initial indexing — no errors expected (use per-file poll to prime the cache)
    poll_diagnostics(&client, Some(&main_rs), has_no_errors).await;

    // 4. Introduce a type error via edit_file
    set_line(&client, &main_rs, 2, "    let x: i32 = \"not an int\";").await;

    // 5. Poll workspace diagnostics until rust-analyzer reports errors.
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

    // Wait for RA to finish initial indexing via workspace-scope polling.
    poll_diagnostics(&client, None, has_no_errors).await;

    set_line(&client, &main_rs, 2, "    let x: i32 = \"not an int\";").await;

    // Poll workspace diagnostics until the edit is picked up by RA.
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
        poll_workspace_diagnostics(&client, |result| file_error_count(result, &main_rs) > 0, Duration::from_secs(15))
            .await;
    assert!(
        file_error_count(&initial, &main_rs) > 0,
        "Expected bootstrap workspace diagnostics to report the initial error. Full result: {initial}"
    );

    set_line(&client, &main_rs, 2, "    let x: i32 = 42;").await;

    let result =
        poll_workspace_diagnostics(&client, |r| file_error_count(r, &main_rs) == 0, Duration::from_secs(15)).await;
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
    // 1. Create a Cargo project with valid code
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

    // 2. Start CodingMcp with LSP enabled
    let (_server_handle, client) = connect_lsp(&project).await;

    // 3. Wait for initial indexing — no errors expected.
    //    Use per-file poll so the diagnostics cache has an entry for this URI.
    poll_diagnostics(&client, Some(&main_rs), has_no_errors).await;

    // 4. Edit the file EXTERNALLY (bypassing MCP tools), introducing a type error
    std::fs::write(
        &main_rs_path,
        r#"fn main() {
    let x: i32 = "not an int";
    println!("{}", x);
}
"#,
    )
    .expect("Failed to write file");

    // 5. Poll workspace diagnostics until file watcher + RA report errors.
    poll_diagnostics(&client, None, has_errors).await;
}

/// Regression test: after an EXTERNAL file edit, a SINGLE workspace-scoped
/// `lsp_check_errors` call (no polling) should return errors. The file watcher keeps the
/// diagnostics cache fresh, so the daemon should simply return whatever is cached.
#[tokio::test]
async fn test_diagnostics_all_files_after_external_edit_single_call() {
    // 1. Create a Cargo project with valid code
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

    // 2. Start CodingMcp with LSP enabled
    let (_server_handle, client) = connect_lsp(&project).await;

    // 3. Wait for initial indexing — no errors expected
    poll_diagnostics(&client, Some(&main_rs), has_no_errors).await;

    // 4. Edit the file EXTERNALLY, introducing a type error
    std::fs::write(
        &main_rs_path,
        r#"fn main() {
    let x: i32 = "not an int";
    println!("{}", x);
}
"#,
    )
    .expect("Failed to write file");

    // 5. Poll workspace diagnostics until file watcher + RA report errors.
    poll_diagnostics(&client, None, has_errors).await;
}

/// Test: External `fs::write` → file watcher → diagnostics queryable
#[tokio::test]
async fn test_external_file_change_produces_diagnostics() {
    // 1. Create a Cargo project with a type error
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

    // 2. Start CodingMcp with LSP enabled
    let (_server_handle, client) = connect_lsp(&project).await;

    // 3. Wait for rust-analyzer to index and report the initial type error
    let result = poll_diagnostics(&client, Some(&main_rs), has_errors).await;
    let errors = result["summary"]["errors"].as_u64().unwrap();
    assert!(errors > 0, "Expected type error diagnostics");

    // 4. Fix the error via direct filesystem write (bypassing MCP tools)
    std::fs::write(
        &main_rs_path,
        r#"fn main() {
    let x: i32 = 42;
    println!("{}", x);
}
"#,
    )
    .expect("Failed to write file");

    // 5. Poll until errors clear (file watcher → didChangeWatchedFiles → RA re-reads)
    poll_diagnostics(&client, Some(&main_rs), has_no_errors).await;

    // 6. Introduce a new error via direct filesystem write
    std::fs::write(
        &main_rs_path,
        r#"fn main() {
    let x: i32 = true;
    println!("{}", x);
}
"#,
    )
    .expect("Failed to write file");

    // 7. Poll until errors reappear
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
    // 1. Create a Cargo project with valid code
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

    // 2. Start CodingMcp with LSP enabled
    let (_server_handle, client) = connect_lsp(&project).await;

    // 3. Wait for RA to finish initial indexing using workspace-scope polling.
    //    We use None (workspace scope) instead of Some(path) (file scope) because
    //    file-scope polling triggers ensure_document_open -> didOpen ->
    //    publishDiagnostics, which would prime the per-file diagnostics cache
    //    and mask the bug we're testing. Workspace-scope polling just calls
    //    wait_for_current_generation() + reads the diagnostics store -- it does
    //    NOT open any documents.
    poll_diagnostics(&client, None, has_no_errors).await;

    // 4. Edit the file EXTERNALLY to introduce a type error.
    //    The file watcher should fire didChangeWatchedFiles.
    std::fs::write(
        &main_rs_path,
        r#"fn main() {
    let x: i32 = "not an int";
    println!("{}", x);
}
"#,
    )
    .expect("Failed to write file");

    // 5. Poll workspace diagnostics until the file watcher detects the change
    //    and RA reports errors.
    poll_diagnostics(&client, None, has_errors).await;
}
