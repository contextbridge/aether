use crate::common::{CodingWorkspace, call_tool, call_tool_error, coding_with_lsp, connect};
use aether_lspd::testing::cargo_project;
use mcp_utils::client::ClientOptions;
use std::os::unix::fs::PermissionsExt;

#[tokio::test]
async fn lsp_check_errors_accepts_flat_file_path_and_infers_file_scope() {
    let project = cargo_project("diag_contract_infers_file_scope");
    project.write("src/main.rs", "fn main() {}\n");

    let client = connect(coding_with_lsp(&project), ClientOptions::default()).await;
    let result = call_tool(
        &client,
        "lsp_check_errors",
        serde_json::json!({
            "filePath": project.path_string("src/main.rs")
        }),
    )
    .await;

    assert_eq!(result["scope"], "file");
}

#[tokio::test]
async fn lsp_check_errors_schema_is_flat_and_has_only_file_path() {
    let project = cargo_project("diag_contract_flat_schema");
    project.write("src/main.rs", "fn main() {}\n");

    let client = connect(coding_with_lsp(&project), ClientOptions::default()).await;
    let tools = client.list_tools().await.expect("list tools");
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
    let project = cargo_project("diag_contract_redundant_scope");
    project.write("src/main.rs", "fn main() {}\n");

    let client = connect(coding_with_lsp(&project), ClientOptions::default()).await;
    let error = call_tool_error(&client, "lsp_check_errors", serde_json::json!({ "scope": "workspace" })).await;

    assert!(error.contains("unknown field `scope`"), "{error}");
}

#[tokio::test]
async fn lsp_check_errors_fails_when_workspace_has_no_active_language_server() {
    let workspace = CodingWorkspace::new_with_lsp().await;
    let error = call_tool_error(&workspace.client, "lsp_check_errors", serde_json::json!({})).await;

    assert!(error.contains("No active LSP clients"), "{error}");
}

#[tokio::test]
async fn lsp_check_errors_returns_typescript_installation_instructions_when_server_fails() {
    let workspace = CodingWorkspace::new_with_lsp().await;
    workspace.write("package.json", "{}\n");
    let index_ts = workspace.write("index.ts", "const value: string = 1;\n");
    workspace.write("node_modules/.bin/tsc", "not executable\n");

    let error =
        call_tool_error(&workspace.client, "lsp_check_errors", serde_json::json!({ "filePath": index_ts })).await;

    assert!(error.contains("Permission denied"), "{error}");
    assert!(error.contains("TypeScript 7 language server"), "{error}");
    assert!(error.contains("npm install --save-dev typescript@^7"), "{error}");
    assert!(error.contains("npm install --global typescript@^7"), "{error}");
}

#[tokio::test]
async fn lsp_check_errors_returns_typescript_installation_instructions_when_tsc_predates_lsp() {
    let workspace = CodingWorkspace::new_with_lsp().await;
    workspace.write("package.json", "{}\n");
    let index_ts = workspace.write("index.ts", "const value: string = 1;\n");
    let tsc = workspace
        .write("node_modules/.bin/tsc", "#!/bin/sh\necho \"error TS5023: Unknown compiler option '--lsp'.\"\nexit 1\n");
    std::fs::set_permissions(&tsc, std::fs::Permissions::from_mode(0o755)).expect("make tsc executable");

    let error =
        call_tool_error(&workspace.client, "lsp_check_errors", serde_json::json!({ "filePath": index_ts })).await;

    assert!(error.contains("failed to complete initialization"), "{error}");
    assert!(error.contains("TypeScript 7 language server"), "{error}");
    assert!(error.contains("npm install --save-dev typescript@^7"), "{error}");
}

#[tokio::test]
async fn lsp_check_errors_rejects_file_scope_parameter() {
    let project = cargo_project("diag_contract_rejects_scope");
    project.write("src/main.rs", "fn main() {}\n");

    let client = connect(coding_with_lsp(&project), ClientOptions::default()).await;
    let error = call_tool_error(
        &client,
        "lsp_check_errors",
        serde_json::json!({
            "scope": "file",
            "filePath": project.path_string("src/main.rs")
        }),
    )
    .await;

    assert!(error.contains("unknown field `scope`"), "{error}");
}

#[tokio::test]
async fn lsp_check_errors_rejects_file_scope_directory_path() {
    let project = cargo_project("diag_contract_file_rejects_directory");
    project.write("src/main.rs", "fn main() {}\n");

    let client = connect(coding_with_lsp(&project), ClientOptions::default()).await;
    let error = call_tool_error(
        &client,
        "lsp_check_errors",
        serde_json::json!({
            "filePath": project.root().to_string_lossy().to_string()
        }),
    )
    .await;

    assert!(error.contains("filePath must point to an existing file"), "{error}");
}
