use crate::common::{CodingWorkspace, McpClientExt, TestResult, call_tool_error};
use aether_lspd::testing::cargo_project;
use mcp_servers::testing::TestWorkspace;
use serde_json::json;
use std::os::unix::fs::PermissionsExt;

#[tokio::test]
async fn lsp_check_errors_accepts_flat_file_path_and_infers_file_scope() -> TestResult {
    let project = rust_project("diag_contract_infers_file_scope").await;

    let result = project.client.call("lsp_check_errors", json!({ "filePath": project.path("src/main.rs") })).await?;

    assert_eq!(result["scope"], "file");
    Ok(())
}

#[tokio::test]
async fn lsp_check_errors_schema_is_flat_and_has_only_file_path() {
    let project = rust_project("diag_contract_flat_schema").await;
    let tools = project.client.list_tools().await.expect("list tools");
    let tool =
        tools.into_iter().find(|tool| tool.name.as_ref() == "lsp_check_errors").expect("lsp_check_errors tool present");

    let schema = serde_json::Value::Object((*tool.input_schema).clone());
    assert_eq!(schema.get("type").and_then(|v| v.as_str()), Some("object"));
    let properties = schema.get("properties").and_then(|v| v.as_object()).expect("schema properties");
    assert_eq!(properties.len(), 1);
    assert!(properties.contains_key("filePath"));
    assert!(!properties.contains_key("scope"));
    assert!(!properties.contains_key("input"));
}

#[tokio::test]
async fn lsp_check_errors_rejects_redundant_scope_parameter() {
    let project = rust_project("diag_contract_redundant_scope").await;
    let error = call_tool_error(&project.client, "lsp_check_errors", json!({ "scope": "workspace" })).await;

    assert!(error.contains("unknown field `scope`"), "{error}");
}

#[tokio::test]
async fn lsp_check_errors_fails_when_workspace_has_no_active_language_server() {
    let workspace = CodingWorkspace::with_lsp(TestWorkspace::new()).await;
    let error = call_tool_error(&workspace.client, "lsp_check_errors", json!({})).await;

    assert!(error.contains("No active LSP clients"), "{error}");
}

#[tokio::test]
async fn lsp_check_errors_returns_typescript_installation_instructions_when_server_fails() {
    let workspace = CodingWorkspace::with_lsp(TestWorkspace::new()).await;
    workspace.write("package.json", "{}\n");
    let index_ts = workspace.write("index.ts", "const value: string = 1;\n");
    workspace.write("node_modules/.bin/tsc", "not executable\n");

    let error = call_tool_error(&workspace.client, "lsp_check_errors", json!({ "filePath": index_ts })).await;

    assert!(error.contains("Permission denied"), "{error}");
    assert!(error.contains("TypeScript 7 language server"), "{error}");
    assert!(error.contains("npm install --save-dev typescript@^7"), "{error}");
    assert!(error.contains("npm install --global typescript@^7"), "{error}");
}

#[tokio::test]
async fn lsp_check_errors_returns_typescript_installation_instructions_when_tsc_predates_lsp() {
    let workspace = CodingWorkspace::with_lsp(TestWorkspace::new()).await;
    workspace.write("package.json", "{}\n");
    let index_ts = workspace.write("index.ts", "const value: string = 1;\n");
    let tsc = workspace
        .write("node_modules/.bin/tsc", "#!/bin/sh\necho \"error TS5023: Unknown compiler option '--lsp'.\"\nexit 1\n");
    std::fs::set_permissions(&tsc, std::fs::Permissions::from_mode(0o755)).expect("make tsc executable");

    let error = call_tool_error(&workspace.client, "lsp_check_errors", json!({ "filePath": index_ts })).await;

    assert!(error.contains("failed to complete initialization"), "{error}");
    assert!(error.contains("TypeScript 7 language server"), "{error}");
    assert!(error.contains("npm install --save-dev typescript@^7"), "{error}");
}

#[tokio::test]
async fn lsp_check_errors_rejects_file_scope_parameter() {
    let project = rust_project("diag_contract_rejects_scope").await;
    let args = json!({ "scope": "file", "filePath": project.path("src/main.rs") });
    let error = call_tool_error(&project.client, "lsp_check_errors", args).await;

    assert!(error.contains("unknown field `scope`"), "{error}");
}

#[tokio::test]
async fn lsp_check_errors_rejects_file_scope_directory_path() {
    let project = rust_project("diag_contract_file_rejects_directory").await;
    let error = call_tool_error(&project.client, "lsp_check_errors", json!({ "filePath": project.root() })).await;

    assert!(error.contains("filePath must point to an existing file"), "{error}");
}

async fn rust_project(name: &str) -> CodingWorkspace {
    CodingWorkspace::with_lsp(cargo_project(name)).await
}
