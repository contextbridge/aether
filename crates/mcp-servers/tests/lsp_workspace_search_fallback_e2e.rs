mod common;

use aether_lspd::testing::{CargoProject, TestProject, use_fake_rust_server_failing_workspace_symbol};
use common::{call_tool, connect_lsp};

#[tokio::test]
async fn workspace_search_falls_back_when_workspace_symbols_fail() {
    use_fake_rust_server_failing_workspace_symbol();
    let project = CargoProject::new("workspace_search_fallback").expect("create project");
    project.add_file("src/lib.rs", "pub fn example_fn() {}\n").expect("add source file");
    let (_server_handle, client) = connect_lsp(&project).await;

    let result =
        call_tool(&client, "lsp_workspace_search", serde_json::json!({ "query": "example_fn", "language": "rust" }))
            .await;

    let entry = result["results"]
        .as_array()
        .and_then(|results| results.iter().find(|entry| entry["name"] == "example_fn"))
        .expect("fallback result");
    assert_eq!(entry["source"], "documentSymbolFallback");
}
