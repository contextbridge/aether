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
async fn workspace_search_recovers_after_language_server_timeout() {
    unsafe { configure_fake_server(LanguageId::TypeScript, &["--wedge-on", "workspace/symbol"]) };

    let root = tempdir().expect("Failed to create project");
    std::fs::write(root.path().join("package.json"), r#"{"name":"workspace-search-recovery"}"#)
        .expect("Failed to write package.json");
    let source_path = root.path().join("example.ts");
    std::fs::write(&source_path, "export function example_fn(): void {}\n").expect("Failed to write example.ts");

    let daemon = TestDaemon::spawn(root.path(), LanguageId::TypeScript, Duration::from_secs(1))
        .await
        .expect("Failed to spawn test daemon");
    let server = CodingMcp::new().with_lsp(root.path().to_path_buf());
    let (server_handle, client) = connect(server, test_client_info()).await.expect("Failed to connect");

    let error = call_tool_error(
        &client,
        "lsp_workspace_search",
        serde_json::json!({ "query": "example_fn", "language": "typescript" }),
    )
    .await;
    assert!(error.contains("timed out after 1s"), "unexpected workspace search error: {error}");

    let result =
        call_tool(&client, "lsp_document", serde_json::json!({ "file_path": source_path.to_string_lossy() })).await;
    assert!(
        result["symbols"]
            .as_array()
            .is_some_and(|symbols| { symbols.iter().any(|symbol| symbol["name"] == "example_fn") })
    );

    drop(client);
    drop(server_handle);
    daemon.shutdown().expect("Failed to shut down test daemon");
    assert!(!socket_path(root.path(), LanguageId::TypeScript).exists());
}
