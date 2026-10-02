#[path = "integration/common/mod.rs"]
mod common;

use aether_lspd::LanguageId;
use aether_lspd::testing::{CargoProject, TestDaemon, TestProject, configure_fake_server};
use common::{call_tool_error, connect_lsp};
use std::time::Duration;

#[tokio::test]
async fn symbol_lookup_propagates_language_server_errors() {
    unsafe { configure_fake_server(LanguageId::Rust, &["--fail-on", "textDocument/definition"]) };
    let project = CargoProject::new("symbol_lookup_errors").expect("create project");
    project.add_file("src/lib.rs", "pub fn example_fn() {}\n").expect("add source file");
    let daemon =
        TestDaemon::spawn(project.root(), LanguageId::Rust, Duration::from_secs(120)).await.expect("start daemon");
    let (server_handle, client) = connect_lsp(&project).await;

    let error = call_tool_error(
        &client,
        "lsp_symbol",
        serde_json::json!({ "operation": "definition", "file_path": "src/lib.rs", "symbol": "example_fn", "line": 1 }),
    )
    .await;

    assert!(error.contains("fake server rejected request"), "unexpected error: {error}");
    drop(client);
    drop(server_handle);
    daemon.shutdown().expect("stop daemon");
}
