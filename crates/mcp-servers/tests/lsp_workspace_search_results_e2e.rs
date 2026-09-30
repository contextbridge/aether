#[path = "integration/common/mod.rs"]
mod common;

use aether_lspd::LanguageId;
use aether_lspd::testing::{CargoProject, TestDaemon, TestProject, configure_fake_server};
use common::{call_tool, connect_lsp};
use std::time::Duration;

#[tokio::test]
async fn workspace_search_returns_only_language_server_symbols() {
    unsafe { configure_fake_server(LanguageId::Rust, &[]) };
    let project = CargoProject::new("workspace_search_results").expect("create project");
    project.add_file("src/lib.rs", "pub struct ExampleStruct;\npub fn example_fn() {}\n").expect("add source file");
    let daemon =
        TestDaemon::spawn(project.root(), LanguageId::Rust, Duration::from_secs(120)).await.expect("start daemon");
    let (server_handle, client) = connect_lsp(&project).await;

    let empty =
        call_tool(&client, "lsp_workspace_search", serde_json::json!({ "query": "ExampleStruct", "language": "rust" }))
            .await;
    assert_eq!(empty["results"], serde_json::json!([]));
    assert_eq!(empty["totalCount"], 0);

    let found =
        call_tool(&client, "lsp_workspace_search", serde_json::json!({ "query": "example_fn", "language": "rust" }))
            .await;
    assert_eq!(found["totalCount"], 1);
    assert_eq!(found["results"][0]["name"], "example_fn");
    assert_eq!(found["results"][0]["containerName"], "module");
    drop(client);
    drop(server_handle);
    daemon.shutdown().expect("stop daemon");
}
