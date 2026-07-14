mod common;

use aether_lspd::testing::{CargoProject, TestProject};
use common::connect_lsp;
use rmcp::RoleClient;
use rmcp::model::{CallToolRequestParams, ClientInfo};
use rmcp::service::RunningService;

fn call_tool_params(name: &str, args: &serde_json::Value) -> CallToolRequestParams {
    CallToolRequestParams::new(name.to_string()).with_arguments(args.as_object().unwrap().clone())
}

async fn call_tool_error(
    client: &RunningService<RoleClient, ClientInfo>,
    name: &str,
    args: &serde_json::Value,
) -> String {
    match client.call_tool(call_tool_params(name, args)).await {
        Ok(result) => {
            assert!(result.is_error.unwrap_or(false), "tool call should fail: {result:?}");
            let content = result.content.first().expect("Expected error content");
            let text = content.as_text().expect("Expected text error content");
            text.text.clone()
        }
        Err(error) => error.to_string(),
    }
}

#[tokio::test]
async fn lsp_check_errors_accepts_flat_file_path_and_infers_file_scope() {
    let project = CargoProject::new("diag_contract_infers_file_scope").expect("Failed to create project");
    project.add_file("src/main.rs", "fn main() {}\n").expect("Failed to add file");

    let (_server_handle, client) = connect_lsp(&project).await;
    let result = client
        .call_tool(call_tool_params(
            "lsp_check_errors",
            &serde_json::json!({
                "filePath": project.file_path_str("src/main.rs")
            }),
        ))
        .await
        .expect("tool call should succeed");

    assert_ne!(result.is_error, Some(true), "tool call failed: {result:?}");
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
    let error = call_tool_error(
        &client,
        "lsp_check_errors",
        &serde_json::json!({
            "scope": "workspace"
        }),
    )
    .await;

    assert!(error.contains("unknown field `scope`"), "{error}");
}

#[tokio::test]
async fn lsp_check_errors_fails_when_workspace_has_no_active_language_server() {
    let project = tempfile::tempdir().expect("create project");
    let server = mcp_servers::coding::CodingMcp::new().with_lsp(project.path().to_path_buf());
    let (_server_handle, client) =
        mcp_utils::testing::connect(server, common::test_client_info()).await.expect("connect coding server");

    let error = call_tool_error(&client, "lsp_check_errors", &serde_json::json!({})).await;

    assert!(error.contains("No active LSP clients"), "{error}");
}

#[tokio::test]
async fn lsp_check_errors_returns_typescript_installation_instructions_when_server_fails() {
    let project = tempfile::tempdir().expect("create project");
    std::fs::write(project.path().join("package.json"), "{}\n").expect("write package.json");
    std::fs::write(project.path().join("index.ts"), "const value: string = 1;\n").expect("write TypeScript file");
    let bin_dir = project.path().join("node_modules/.bin");
    std::fs::create_dir_all(&bin_dir).expect("create bin directory");
    std::fs::write(bin_dir.join("typescript-language-server"), "not executable\n")
        .expect("write unavailable language server");

    let server = mcp_servers::coding::CodingMcp::new().with_lsp(project.path().to_path_buf());
    let (_server_handle, client) =
        mcp_utils::testing::connect(server, common::test_client_info()).await.expect("connect coding server");
    let error = call_tool_error(
        &client,
        "lsp_check_errors",
        &serde_json::json!({ "filePath": project.path().join("index.ts") }),
    )
    .await;

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
        &serde_json::json!({
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
        &serde_json::json!({
            "filePath": project.root().to_string_lossy().to_string()
        }),
    )
    .await;

    assert!(error.contains("filePath must point to an existing file"), "{error}");
}
