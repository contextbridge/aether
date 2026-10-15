#[path = "integration/common/mod.rs"]
mod common;

use aether_lspd::LanguageId;
use aether_lspd::testing::{TestDaemon, cargo_project, configure_fake_server};
use common::{CodingWorkspace, call_tool_error};
use std::time::Duration;

#[tokio::test]
async fn symbol_lookup_propagates_language_server_errors() {
    unsafe { configure_fake_server(LanguageId::Rust, &["--fail-on", "textDocument/definition"]) };
    let project = cargo_project("symbol_lookup_errors");
    project.write("src/lib.rs", "pub fn example_fn() {}\n");
    let daemon =
        TestDaemon::spawn(project.root(), LanguageId::Rust, Duration::from_secs(120)).await.expect("start daemon");
    let project = CodingWorkspace::with_lsp(project).await;

    let error = call_tool_error(
        &project.client,
        "lsp_symbol",
        serde_json::json!({ "operation": "definition", "file_path": "src/lib.rs", "symbol": "example_fn", "line": 1 }),
    )
    .await;

    assert!(error.contains("fake server rejected request"), "unexpected error: {error}");
    project.client.close().await;
    daemon.shutdown().expect("stop daemon");
}
