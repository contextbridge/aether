#[path = "integration/common/mod.rs"]
mod common;

use aether_lspd::testing::{TestDaemon, configure_fake_server};
use aether_lspd::{LanguageId, socket_path};
use common::{call_tool, call_tool_error, test_client_info};
use mcp_servers::coding::CodingMcp;
use mcp_utils::testing::connect;
use std::time::Duration;
use tempfile::tempdir;

#[tokio::test]
async fn symbol_lookup_recovers_after_language_server_timeout() {
    unsafe { configure_fake_server(LanguageId::TypeScript, &["--wedge-on", "textDocument/definition"]) };

    let root = tempdir().expect("Failed to create project");
    std::fs::write(root.path().join("package.json"), r#"{"name":"symbol-lookup-recovery"}"#)
        .expect("Failed to write package.json");
    std::fs::write(root.path().join("example.ts"), "export function example_fn(): void {}\n")
        .expect("Failed to write example.ts");

    let daemon = TestDaemon::spawn(root.path(), LanguageId::TypeScript, Duration::from_secs(1))
        .await
        .expect("Failed to spawn test daemon");
    let server = CodingMcp::new().with_lsp(root.path().to_path_buf());
    let (server_handle, client) = connect(server, test_client_info()).await.expect("Failed to connect");
    let lookup = |operation: &str| serde_json::json!({ "operation": operation, "file_path": "example.ts", "symbol": "example_fn", "line": 1 });

    let error = call_tool_error(&client, "lsp_symbol", lookup("definition")).await;
    assert!(error.contains("timed out after 1s"), "unexpected definition error: {error}");

    let result = call_tool(&client, "lsp_symbol", lookup("hover")).await;
    let hover = result["hoverContents"].as_str().unwrap_or_default();
    assert!(hover.contains("export function example_fn"), "{result}");

    drop(client);
    drop(server_handle);
    daemon.shutdown().expect("Failed to shut down test daemon");
    assert!(!socket_path(root.path(), LanguageId::TypeScript).exists());
}
