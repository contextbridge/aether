#[path = "integration/common/mod.rs"]
mod common;

use aether_lspd::LanguageId;
use aether_lspd::testing::{CargoProject, TestDaemon, TestProject, use_fake_rust_server_failing_workspace_symbol};
use common::{call_tool_error, connect_lsp};
use std::time::Duration;

#[tokio::test]
async fn workspace_search_propagates_language_server_errors() {
    use_fake_rust_server_failing_workspace_symbol();
    let project = CargoProject::new("workspace_search_errors").expect("create project");
    project.add_file("src/lib.rs", "pub fn example_fn() {}\n").expect("add source file");
    let daemon =
        TestDaemon::spawn(project.root(), LanguageId::Rust, Duration::from_secs(120)).await.expect("start daemon");
    let (server_handle, client) = connect_lsp(&project).await;

    let error = call_tool_error(
        &client,
        "lsp_workspace_search",
        serde_json::json!({ "query": "example_fn", "language": "rust" }),
    )
    .await;

    assert!(error.contains("fake server rejected request"), "unexpected error: {error}");
    drop(client);
    drop(server_handle);
    daemon.shutdown().expect("stop daemon");
}
