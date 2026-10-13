#[path = "integration/common/mod.rs"]
mod common;

use aether_lspd::LanguageId;
use aether_lspd::testing::{TestDaemon, cargo_project, configure_fake_server};
use common::{call_tool_error, coding_with_lsp, connect};
use mcp_utils::client::ClientOptions;
use std::time::Duration;

#[tokio::test]
async fn symbol_lookup_propagates_language_server_errors() {
    unsafe { configure_fake_server(LanguageId::Rust, &["--fail-on", "textDocument/definition"]) };
    let project = cargo_project("symbol_lookup_errors");
    project.write("src/lib.rs", "pub fn example_fn() {}\n");
    let daemon =
        TestDaemon::spawn(project.root(), LanguageId::Rust, Duration::from_secs(120)).await.expect("start daemon");
    let client = connect(coding_with_lsp(&project), ClientOptions::default()).await;

    let error = call_tool_error(
        &client,
        "lsp_symbol",
        serde_json::json!({ "operation": "definition", "file_path": "src/lib.rs", "symbol": "example_fn", "line": 1 }),
    )
    .await;

    assert!(error.contains("fake server rejected request"), "unexpected error: {error}");
    client.close().await;
    daemon.shutdown().expect("stop daemon");
}
