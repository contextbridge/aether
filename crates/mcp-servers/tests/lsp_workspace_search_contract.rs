mod common;

use mcp_servers::coding::CodingMcp;
use rmcp::model::CallToolRequestParams;

#[tokio::test]
async fn lsp_workspace_search_schema_requires_one_language() {
    let workspace = tempfile::tempdir().expect("create workspace");
    let server = CodingMcp::new().with_lsp(workspace.path().to_path_buf());
    let (_server_handle, client) =
        mcp_utils::testing::connect(server, common::test_client_info()).await.expect("connect coding server");

    let tools = client.peer().list_all_tools().await.expect("list tools");
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
    let workspace = tempfile::tempdir().expect("create workspace");
    let server = CodingMcp::new().with_lsp(workspace.path().to_path_buf());
    let (_server_handle, client) =
        mcp_utils::testing::connect(server, common::test_client_info()).await.expect("connect coding server");

    let result = client
        .call_tool(
            CallToolRequestParams::new("lsp_workspace_search")
                .with_arguments(serde_json::json!({ "query": "AppState" }).as_object().unwrap().clone()),
        )
        .await;
    let error = match result {
        Ok(result) => result
            .content
            .first()
            .and_then(|content| content.as_text())
            .map(|text| text.text.clone())
            .unwrap_or_default(),
        Err(error) => error.to_string(),
    };

    assert!(error.contains("language"), "{error}");
}

#[tokio::test]
async fn lsp_workspace_search_rejects_all_language() {
    let workspace = tempfile::tempdir().expect("create workspace");
    let server = CodingMcp::new().with_lsp(workspace.path().to_path_buf());
    let (_server_handle, client) =
        mcp_utils::testing::connect(server, common::test_client_info()).await.expect("connect coding server");

    let result =
        client
            .call_tool(CallToolRequestParams::new("lsp_workspace_search").with_arguments(
                serde_json::json!({ "query": "AppState", "language": "all" }).as_object().unwrap().clone(),
            ))
            .await;
    let error = match result {
        Ok(result) => result
            .content
            .first()
            .and_then(|content| content.as_text())
            .map(|text| text.text.clone())
            .unwrap_or_default(),
        Err(error) => error.to_string(),
    };

    assert!(error.contains("unknown variant `all`"), "{error}");
}
