use crate::common::{CodingWorkspace, call_tool_error};
use aether_lspd::LanguageId;
use aether_lspd::testing::TestDaemon;
use mcp_servers::testing::TestWorkspace;
use serde_json::json;
use std::os::unix::fs::PermissionsExt;

#[tokio::test]
async fn lsp_check_errors_schema_is_flat_and_has_only_file_path() {
    let workspace = CodingWorkspace::with_lsp(TestWorkspace::new()).await;
    let tools = workspace.client.list_tools().await.expect("list tools");
    let tool =
        tools.into_iter().find(|tool| tool.name.as_ref() == "lsp_check_errors").expect("lsp_check_errors tool present");

    let schema = serde_json::Value::Object((*tool.input_schema).clone());
    assert_eq!(schema.get("type").and_then(|v| v.as_str()), Some("object"));
    let properties = schema.get("properties").and_then(|v| v.as_object()).expect("schema properties");
    assert_eq!(properties.len(), 1);
    assert!(properties.contains_key("filePath"));
}

#[tokio::test]
async fn lsp_check_errors_rejects_scope_parameters() {
    let workspace = CodingWorkspace::with_lsp(TestWorkspace::new().file("src/main.rs", "fn main() {}\n")).await;
    let main_rs = workspace.path("src/main.rs");

    for args in [json!({ "scope": "workspace" }), json!({ "scope": "file", "filePath": main_rs })] {
        let error = call_tool_error(&workspace.client, "lsp_check_errors", args).await;
        assert!(error.contains("unknown field `scope`"), "{error}");
    }
}

#[tokio::test]
async fn lsp_check_errors_rejects_file_scope_directory_path() {
    let workspace = CodingWorkspace::with_lsp(TestWorkspace::new()).await;
    let error = call_tool_error(&workspace.client, "lsp_check_errors", json!({ "filePath": workspace.root() })).await;

    assert!(error.contains("filePath must point to an existing file"), "{error}");
}

#[tokio::test]
async fn lsp_check_errors_fails_when_workspace_has_no_active_language_server() {
    let workspace = CodingWorkspace::with_lsp(TestWorkspace::new()).await;
    let error = call_tool_error(&workspace.client, "lsp_check_errors", json!({})).await;

    assert!(error.contains("No active LSP clients"), "{error}");
}

#[tokio::test]
async fn lsp_check_errors_returns_typescript_installation_instructions_when_server_fails() {
    let workspace = typescript_workspace("not executable\n", 0o644).await;

    let error = check_index_ts(&workspace).await;

    assert!(error.contains("Permission denied"), "{error}");
    assert!(error.contains("TypeScript 7 language server"), "{error}");
    assert!(error.contains("npm install --save-dev typescript@^7"), "{error}");
    assert!(error.contains("npm install --global typescript@^7"), "{error}");
}

#[tokio::test]
async fn lsp_check_errors_returns_typescript_installation_instructions_when_tsc_predates_lsp() {
    let tsc = "#!/bin/sh\necho \"error TS5023: Unknown compiler option '--lsp'.\"\nexit 1\n";
    let workspace = typescript_workspace(tsc, 0o755).await;

    let error = check_index_ts(&workspace).await;

    assert!(error.contains("failed to complete initialization"), "{error}");
    assert!(error.contains("TypeScript 7 language server"), "{error}");
    assert!(error.contains("npm install --save-dev typescript@^7"), "{error}");
}

/// A TypeScript workspace whose local `tsc` is `script` with permissions `mode`.
async fn typescript_workspace(script: &str, mode: u32) -> CodingWorkspace {
    let workspace = TestWorkspace::new()
        .file("package.json", "{}\n")
        .file("index.ts", "const value: string = 1;\n")
        .file("node_modules/.bin/tsc", script);
    let tsc = workspace.path("node_modules/.bin/tsc");
    std::fs::set_permissions(tsc, std::fs::Permissions::from_mode(mode)).expect("set tsc permissions");
    let daemon = TestDaemon::builder(workspace.root(), LanguageId::TypeScript);
    CodingWorkspace::with_daemon(workspace, daemon).await
}

async fn check_index_ts(workspace: &CodingWorkspace) -> String {
    call_tool_error(&workspace.client, "lsp_check_errors", json!({ "filePath": workspace.path("index.ts") })).await
}
