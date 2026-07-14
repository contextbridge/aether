mod common;

use common::{CodingWorkspace, call_tool_error};

#[tokio::test]
async fn lsp_workspace_search_schema_requires_one_language() {
    let workspace = CodingWorkspace::new_with_lsp().await.expect("create workspace");

    let tools = workspace.client.raw().peer().list_all_tools().await.expect("list tools");
    let tool = tools
        .into_iter()
        .find(|tool| tool.name.as_ref() == "lsp_workspace_search")
        .expect("lsp_workspace_search tool present");
    let schema = serde_json::Value::Object((*tool.input_schema).clone());
    let properties = schema.get("properties").and_then(|value| value.as_object()).expect("schema properties");
    let required = schema.get("required").and_then(|value| value.as_array()).expect("required properties");

    assert!(properties.contains_key("language"));
    assert!(required.iter().any(|field| field == "language"));
}

#[tokio::test]
async fn lsp_workspace_search_rejects_missing_language() {
    let workspace = CodingWorkspace::new_with_lsp().await.expect("create workspace");
    let error =
        call_tool_error(workspace.client.raw(), "lsp_workspace_search", serde_json::json!({ "query": "AppState" }))
            .await;

    assert!(error.contains("language"), "{error}");
}

#[tokio::test]
async fn lsp_workspace_search_rejects_all_language() {
    let workspace = CodingWorkspace::new_with_lsp().await.expect("create workspace");
    let error = call_tool_error(
        workspace.client.raw(),
        "lsp_workspace_search",
        serde_json::json!({ "query": "AppState", "language": "all" }),
    )
    .await;

    assert!(error.contains("unknown variant `all`"), "{error}");
}
