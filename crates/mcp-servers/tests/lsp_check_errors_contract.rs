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
async fn lsp_check_errors_rejects_unwrapped_arguments() {
    let project = CargoProject::new("diag_contract_rejects_unwrapped").expect("Failed to create project");
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

    assert!(error.contains("expected `input`"), "{error}");
    assert!(error.contains("scope"), "{error}");
}

#[tokio::test]
async fn lsp_check_errors_schema_wraps_discriminated_union_in_object() {
    let project = CargoProject::new("diag_contract_schema_wrapper").expect("Failed to create project");
    project.add_file("src/main.rs", "fn main() {}\n").expect("Failed to add file");

    let (_server_handle, client) = connect_lsp(&project).await;
    let tools = client.peer().list_all_tools().await.expect("list tools");
    let tool =
        tools.into_iter().find(|tool| tool.name.as_ref() == "lsp_check_errors").expect("lsp_check_errors tool present");

    let schema = serde_json::Value::Object((*tool.input_schema).clone());
    assert_eq!(schema.get("type").and_then(|v| v.as_str()), Some("object"));

    let input = schema.get("properties").and_then(|v| v.get("input")).expect("top-level input property");
    assert_eq!(input.get("$ref").and_then(|v| v.as_str()), Some("#/$defs/LspDiagnosticsInput"));
    let variants = schema
        .get("$defs")
        .and_then(|v| v.get("LspDiagnosticsInput"))
        .and_then(|v| v.get("oneOf"))
        .and_then(|v| v.as_array())
        .expect("wrapped discriminated union");

    assert_eq!(variants.len(), 2);
    assert!(variants.iter().any(|variant| {
        variant.get("properties").and_then(|v| v.get("scope")).and_then(|v| v.get("const")).and_then(|v| v.as_str())
            == Some("workspace")
    }));
    assert!(variants.iter().any(|variant| {
        variant.get("properties").and_then(|v| v.get("scope")).and_then(|v| v.get("const")).and_then(|v| v.as_str())
            == Some("file")
    }));
}

#[tokio::test]
async fn lsp_check_errors_accepts_stringified_workspace_input() {
    let project = CargoProject::new("diag_contract_stringified_workspace").expect("Failed to create project");
    project.add_file("src/main.rs", "fn main() {}\n").expect("Failed to add file");

    let (_server_handle, client) = connect_lsp(&project).await;
    let result = client
        .call_tool(call_tool_params(
            "lsp_check_errors",
            &serde_json::json!({
                "input": "{\"scope\":\"workspace\"}"
            }),
        ))
        .await
        .expect("tool call should succeed");

    assert_ne!(result.is_error, Some(true), "tool call failed: {result:?}");
}

#[tokio::test]
async fn lsp_check_errors_rejects_workspace_scope_with_file_path() {
    let project = CargoProject::new("diag_contract_workspace_rejects_file_path").expect("Failed to create project");
    project.add_file("src/main.rs", "fn main() {}\n").expect("Failed to add file");

    let (_server_handle, client) = connect_lsp(&project).await;
    let error = call_tool_error(
        &client,
        "lsp_check_errors",
        &serde_json::json!({
            "input": {
                "scope": "workspace",
                "filePath": project.file_path_str("src/main.rs")
            }
        }),
    )
    .await;

    assert!(error.contains("unknown field `filePath`"), "{error}");
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
            "input": {
                "scope": "file",
                "filePath": project.root().to_string_lossy().to_string()
            }
        }),
    )
    .await;

    assert!(error.contains("filePath must point to an existing file"), "{error}");
}
