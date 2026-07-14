mod common;

use aether_lspd::testing::{CargoProject, TestProject};
use common::{CodingWorkspace, call_tool, call_tool_error, connect_lsp};

#[tokio::test]
async fn lsp_check_errors_accepts_flat_file_path_and_infers_file_scope() {
    let project = CargoProject::new("diag_contract_infers_file_scope").expect("Failed to create project");
    project.add_file("src/main.rs", "fn main() {}\n").expect("Failed to add file");

    let (_server_handle, client) = connect_lsp(&project).await;
    let result = call_tool(
        &client,
        "lsp_check_errors",
        serde_json::json!({
            "filePath": project.file_path_str("src/main.rs")
        }),
    )
    .await;

    assert_eq!(result["scope"], "file");
}

#[tokio::test]
async fn lsp_check_errors_schema_is_flat_and_has_only_file_path() {
    let project = CargoProject::new("diag_contract_flat_schema").expect("Failed to create project");
    project.add_file("src/main.rs", "fn main() {}\n").expect("Failed to add file");

    let (_server_handle, client) = connect_lsp(&project).await;
    let tools = client.peer().list_all_tools().await.expect("list tools");
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
    let project = CargoProject::new("diag_contract_stringified_workspace").expect("Failed to create project");
    project.add_file("src/main.rs", "fn main() {}\n").expect("Failed to add file");

    let (_server_handle, client) = connect_lsp(&project).await;
    let error = call_tool_error(&client, "lsp_check_errors", serde_json::json!({ "scope": "workspace" })).await;

    assert!(error.contains("unknown field `scope`"), "{error}");
}

#[tokio::test]
async fn lsp_check_errors_fails_when_workspace_has_no_active_language_server() {
    let workspace = CodingWorkspace::new_with_lsp().await.expect("create workspace");
    let error = call_tool_error(workspace.client.raw(), "lsp_check_errors", serde_json::json!({})).await;

    assert!(error.contains("No active LSP clients"), "{error}");
}

#[tokio::test]
async fn lsp_check_errors_returns_typescript_installation_instructions_when_server_fails() {
    let workspace = CodingWorkspace::new_with_lsp().await.expect("create workspace");
    workspace.write("package.json", "{}\n").expect("write package.json");
    let index_ts = workspace.write("index.ts", "const value: string = 1;\n").expect("write TypeScript file");
    workspace
        .write("node_modules/.bin/typescript-language-server", "not executable\n")
        .expect("write unavailable language server");

    let error =
        call_tool_error(workspace.client.raw(), "lsp_check_errors", serde_json::json!({ "filePath": index_ts })).await;

    assert!(error.contains("Permission denied"), "{error}");
    assert!(error.contains("TypeScript language server"), "{error}");
    assert!(error.contains("npm install --save-dev typescript typescript-language-server"), "{error}");
    assert!(error.contains("npm install --global typescript typescript-language-server"), "{error}");
}

#[tokio::test]
async fn lsp_check_errors_rejects_file_scope_parameter() {
    let project = CargoProject::new("diag_contract_rejects_scope").expect("Failed to create project");
    project.add_file("src/main.rs", "fn main() {}\n").expect("Failed to add file");

    let (_server_handle, client) = connect_lsp(&project).await;
    let error = call_tool_error(
        &client,
        "lsp_check_errors",
        serde_json::json!({
            "scope": "file",
            "filePath": project.file_path_str("src/main.rs")
        }),
    )
    .await;

    assert!(error.contains("unknown field `scope`"), "{error}");
}

#[tokio::test]
async fn lsp_check_errors_rejects_file_scope_directory_path() {
    let project = CargoProject::new("diag_contract_file_rejects_directory").expect("Failed to create project");
    project.add_file("src/main.rs", "fn main() {}\n").expect("Failed to add file");

    let (_server_handle, client) = connect_lsp(&project).await;
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
