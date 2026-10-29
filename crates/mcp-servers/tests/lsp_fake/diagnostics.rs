use crate::common::{CodingWorkspace, TestResult, cargo_project, has_errors, has_no_errors};
use crate::fake_lsp;
use clankerdiff_lspd::LanguageId;
use serde_json::Value;
use std::fs::canonicalize;
use std::path::{Path, PathBuf};

const MAIN_RS: &str = "src/main.rs";
const CLEAN: &str = "42";
const BROKEN: &str = "error";

#[tokio::test]
async fn file_checks_follow_mcp_edits() -> TestResult {
    let project = rust_project(BROKEN).await;

    let result = project.check_errors(MAIN_RS).await?;
    assert_eq!(result["scope"], "file");
    assert!(has_errors(&result), "{result}");

    project.read_then_edit(MAIN_RS, BROKEN, CLEAN).await?;
    assert!(has_no_errors(&project.check_errors(MAIN_RS).await?));

    project.read_then_edit(MAIN_RS, CLEAN, BROKEN).await?;
    assert!(has_errors(&project.check_errors(MAIN_RS).await?));
    Ok(())
}

#[tokio::test]
async fn file_checks_follow_external_writes() -> TestResult {
    let project = rust_project(BROKEN).await;
    assert!(has_errors(&project.check_errors(MAIN_RS).await?));

    project.write(MAIN_RS, &main_rs(CLEAN));
    assert!(has_no_errors(&project.check_errors(MAIN_RS).await?));

    project.write(MAIN_RS, &main_rs(BROKEN));
    assert!(has_errors(&project.check_errors(MAIN_RS).await?));
    Ok(())
}

#[tokio::test]
async fn workspace_checks_follow_mcp_edits() -> TestResult {
    let project = rust_project(CLEAN).await;
    let main_rs = project.path(MAIN_RS);

    project.poll_workspace_errors(has_no_errors).await;
    project.read_then_edit(MAIN_RS, CLEAN, BROKEN).await?;
    project.poll_workspace_errors(|result| file_error_count(result, &main_rs) > 0).await;
    project.read_then_edit(MAIN_RS, BROKEN, CLEAN).await?;
    project.poll_workspace_errors(|result| file_error_count(result, &main_rs) == 0).await;
    Ok(())
}

#[tokio::test]
async fn workspace_checks_pick_up_external_writes() {
    let project = rust_project(CLEAN).await;

    project.poll_workspace_errors(has_no_errors).await;
    project.write(MAIN_RS, &main_rs(BROKEN));
    project.poll_workspace_errors(has_errors).await;
}

async fn rust_project(value: &str) -> CodingWorkspace {
    fake_lsp(cargo_project("diagnostics").file(MAIN_RS, main_rs(value)), LanguageId::Rust).await
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
