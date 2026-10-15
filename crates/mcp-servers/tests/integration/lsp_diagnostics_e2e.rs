//!

use crate::common::{CodingWorkspace, TestResult, has_errors, has_no_errors};
use aether_lspd::testing::cargo_project;
use serde_json::Value;
use std::fs::canonicalize;
use std::path::{Path, PathBuf};

const MAIN_RS: &str = "src/main.rs";
const NOT_AN_INT: &str = "\"not an int\"";

#[tokio::test]
async fn test_mcp_edit_produces_diagnostics() -> TestResult {
    let project = rust_project("mcp_edit_diag", NOT_AN_INT).await;

    project.poll_errors(Some(MAIN_RS), has_errors).await;
    project.read_then_edit(MAIN_RS, NOT_AN_INT, "42").await?;
    project.poll_errors(Some(MAIN_RS), has_no_errors).await;
    project.read_then_edit(MAIN_RS, "42", "true").await?;
    project.poll_errors(Some(MAIN_RS), has_errors).await;
    Ok(())
}

#[tokio::test]
async fn test_diagnostics_available_after_edit_without_polling() -> TestResult {
    let project = rust_project("diag_after_edit_no_poll", "42").await;

    project.poll_errors(Some(MAIN_RS), has_no_errors).await;
    project.read_then_edit(MAIN_RS, "42", NOT_AN_INT).await?;
    project.poll_errors(Some(MAIN_RS), has_errors).await;
    Ok(())
}

#[tokio::test]
async fn test_diagnostics_all_files_after_edit() -> TestResult {
    let project = rust_project("diag_all_files", "42").await;

    project.poll_errors(Some(MAIN_RS), has_no_errors).await;
    project.read_then_edit(MAIN_RS, "42", NOT_AN_INT).await?;
    project.poll_errors(None, has_errors).await;
    Ok(())
}

#[tokio::test]
async fn test_workspace_diagnostics_after_edit_without_file_check() -> TestResult {
    let project = rust_project("diag_workspace_after_edit", "42").await;
    let main_rs = project.path(MAIN_RS);

    project.poll_errors(None, has_no_errors).await;
    project.read_then_edit(MAIN_RS, "42", NOT_AN_INT).await?;
    project.poll_errors(None, |result| file_error_count(result, &main_rs) > 0).await;
    Ok(())
}

#[tokio::test]
async fn test_workspace_diagnostics_clear_after_fix_without_file_check() -> TestResult {
    let project = rust_project("diag_workspace_fix", NOT_AN_INT).await;
    let main_rs = project.path(MAIN_RS);

    project.poll_errors(None, |result| file_error_count(result, &main_rs) > 0).await;
    project.read_then_edit(MAIN_RS, NOT_AN_INT, "42").await?;
    project.poll_errors(None, |result| file_error_count(result, &main_rs) == 0).await;
    Ok(())
}

#[tokio::test]
async fn test_diagnostics_all_files_after_external_edit() {
    let project = rust_project("diag_all_ext_edit", "42").await;

    project.poll_errors(Some(MAIN_RS), has_no_errors).await;
    project.write(MAIN_RS, &main_rs(NOT_AN_INT));
    project.poll_errors(None, has_errors).await;
}

#[tokio::test]
async fn test_external_file_change_produces_diagnostics() {
    let project = rust_project("ext_write_diag", NOT_AN_INT).await;

    project.poll_errors(Some(MAIN_RS), has_errors).await;
    project.write(MAIN_RS, &main_rs("42"));
    project.poll_errors(Some(MAIN_RS), has_no_errors).await;
    project.write(MAIN_RS, &main_rs("true"));
    project.poll_errors(Some(MAIN_RS), has_errors).await;
}

#[tokio::test]
async fn test_diagnostics_all_files_discovers_file_watcher_uris() {
    let project = rust_project("diag_fw_discover", "42").await;

    project.poll_errors(None, has_no_errors).await;
    project.write(MAIN_RS, &main_rs(NOT_AN_INT));
    project.poll_errors(None, has_errors).await;
}

async fn rust_project(name: &str, value: &str) -> CodingWorkspace {
    CodingWorkspace::with_lsp(cargo_project(name).file(MAIN_RS, main_rs(value))).await
}

fn main_rs(value: &str) -> String {
    format!("fn main() {{\n    let x: i32 = {value};\n    println!(\"{{}}\", x);\n}}\n")
}

fn file_error_count(result: &Value, file: &Path) -> usize {
    let root = PathBuf::from(result["workspaceRoot"].as_str().unwrap_or_default());
    let file = canonical(file);
    result["diagnostics"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(path, _)| canonical(&root.join(path)) == file)
        .flat_map(|(_, diagnostics)| diagnostics.as_array().into_iter().flatten())
        .filter(|diagnostic| diagnostic.as_str().is_some_and(|diagnostic| diagnostic.contains(" error")))
        .count()
}

fn canonical(path: &Path) -> PathBuf {
    canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}
