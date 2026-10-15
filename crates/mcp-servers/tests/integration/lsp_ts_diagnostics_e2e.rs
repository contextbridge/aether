use crate::common::{CodingWorkspace, McpClientExt, TestResult, has_errors, has_no_errors};
use aether_lspd::testing::node_project;
use serde_json::{Value, json};

const INDEX_TS: &str = "src/index.ts";
const NOT_A_NUMBER: &str = "\"not a number\"";

#[tokio::test]
async fn test_ts_mcp_edit_produces_diagnostics() -> TestResult {
    let project = ts_project("ts_mcp_edit_diag", NOT_A_NUMBER).await;

    let result = check_errors(&project).await?;
    assert!(has_errors(&result), "Expected type error diagnostics: {result}");

    project.read_then_edit(INDEX_TS, NOT_A_NUMBER, "42").await?;
    let result = check_errors(&project).await?;
    assert!(has_no_errors(&result), "Expected no errors after fixing the bug: {result}");

    project.read_then_edit(INDEX_TS, "42", "true").await?;
    let result = check_errors(&project).await?;
    assert!(has_errors(&result), "Expected type error after re-introducing bug: {result}");
    Ok(())
}

#[tokio::test]
async fn test_ts_external_file_change_produces_diagnostics() -> TestResult {
    let project = ts_project("ts_ext_write_diag", NOT_A_NUMBER).await;

    let result = check_errors(&project).await?;
    assert!(has_errors(&result), "Expected type error diagnostics: {result}");

    project.write(INDEX_TS, &index_ts("42"));
    let result = check_errors(&project).await?;
    assert!(has_no_errors(&result), "Expected no errors after external fix: {result}");

    project.write(INDEX_TS, &index_ts("true"));
    let result = check_errors(&project).await?;
    assert!(has_errors(&result), "Expected type error after external write: {result}");
    Ok(())
}

#[tokio::test]
async fn test_ts_diagnostics_after_edit_without_polling() -> TestResult {
    let project = ts_project("ts_diag_no_poll", "42").await;

    project.read_then_edit(INDEX_TS, "42", NOT_A_NUMBER).await?;

    let result = check_errors(&project).await?;
    assert!(has_errors(&result), "Expected diagnostics after edit + single lsp_check_errors call: {result}");
    Ok(())
}

async fn ts_project(name: &str, value: &str) -> CodingWorkspace {
    CodingWorkspace::with_lsp(node_project(name).file(INDEX_TS, index_ts(value))).await
}

fn index_ts(value: &str) -> String {
    format!("const x: number = {value};\nconsole.log(x);\n")
}

async fn check_errors(project: &CodingWorkspace) -> TestResult<Value> {
    project.client.call("lsp_check_errors", json!({ "filePath": project.path(INDEX_TS) })).await
}
