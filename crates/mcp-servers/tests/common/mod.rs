//! Shared test helpers for LSP end-to-end tests.

#![allow(dead_code)]

use aether_lspd::testing::TestProject;
use mcp_servers::coding::CodingMcp;
use mcp_utils::testing::connect;
use rmcp::RoleClient;
use rmcp::model::{CallToolRequestParams, ClientCapabilities, ClientInfo, Implementation};
use rmcp::service::RunningService;
use std::time::{Duration, Instant};

/// Default timeout for polling operations (60 seconds).
const POLL_TIMEOUT: Duration = Duration::from_mins(1);
const POLL_INTERVAL: Duration = Duration::from_millis(500);

pub fn test_client_info() -> ClientInfo {
    ClientInfo::new(ClientCapabilities::default(), Implementation::new("lsp-e2e-test", "0.1.0"))
}

/// Connect a `CodingMcp` server to a test project with LSP enabled.
///
/// Returns `(server_handle, client)`. The caller must keep the server handle
/// alive (bind it to `_server_handle`) for the connection to stay open.
pub async fn connect_lsp(
    project: &impl TestProject,
) -> (rmcp::service::RunningService<rmcp::RoleServer, CodingMcp>, RunningService<RoleClient, ClientInfo>) {
    let server = CodingMcp::new().with_lsp(project.root().to_path_buf());
    connect(server, test_client_info()).await.expect("Failed to connect")
}

/// Call an MCP tool and parse the JSON response from the first text content block.
pub async fn call_tool(
    client: &RunningService<RoleClient, ClientInfo>,
    name: &str,
    args: serde_json::Value,
) -> serde_json::Value {
    try_call_tool(client, name, args).await.unwrap_or_else(|| panic!("Tool '{name}' did not return valid JSON"))
}

/// Try to call an MCP tool and parse JSON. Returns `None` if the tool returns
/// a non-JSON response (e.g. during LSP startup when the server isn't ready yet).
pub async fn try_call_tool(
    client: &RunningService<RoleClient, ClientInfo>,
    name: &str,
    args: serde_json::Value,
) -> Option<serde_json::Value> {
    let name_owned = name.to_string();
    let result = match client
        .call_tool(CallToolRequestParams::new(name_owned).with_arguments(args.as_object().unwrap().clone()))
        .await
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[try_call_tool] {name} RPC error: {e}");
            return None;
        }
    };

    let Some(text) = result.content.first().and_then(|c| c.as_text()) else {
        eprintln!("[try_call_tool] {name} no text content in response");
        return None;
    };

    if let Ok(v) = serde_json::from_str(&text.text) {
        Some(v)
    } else {
        eprintln!("[try_call_tool] {name} non-JSON response: {}", text.text);
        None
    }
}

/// Poll `lsp_check_errors` until a predicate is satisfied or timeout is reached.
pub async fn poll_diagnostics(
    client: &RunningService<RoleClient, ClientInfo>,
    file_path: Option<&str>,
    predicate: impl Fn(&serde_json::Value) -> bool,
) -> serde_json::Value {
    let args = match file_path {
        Some(path) => serde_json::json!({ "input": { "scope": "file", "filePath": path } }),
        None => serde_json::json!({ "input": { "scope": "workspace" } }),
    };
    poll_lsp_tool(client, "lsp_check_errors", args, predicate).await
}

/// Poll an LSP tool until its result satisfies a predicate or timeout is reached.
///
/// Useful for operations like hover/definition that may return empty results
/// while the LSP server is still indexing. Gracefully handles errors during
/// LSP startup (e.g. "No LSP configured for this file type" before the
/// background spawning completes).
pub async fn poll_lsp_tool(
    client: &RunningService<RoleClient, ClientInfo>,
    tool_name: &str,
    args: serde_json::Value,
    predicate: impl Fn(&serde_json::Value) -> bool,
) -> serde_json::Value {
    let start = Instant::now();
    let mut last_result = None;

    while start.elapsed() < POLL_TIMEOUT {
        if let Some(result) = try_call_tool(client, tool_name, args.clone()).await {
            if predicate(&result) {
                return result;
            }
            last_result = Some(result);
        }

        tokio::time::sleep(POLL_INTERVAL).await;
    }

    panic!(
        "poll_lsp_tool({tool_name}) timed out after {POLL_TIMEOUT:?}. Last result: {}",
        last_result.as_ref().map_or_else(|| "(no valid response)".to_string(), ToString::to_string)
    );
}

fn error_count(result: &serde_json::Value) -> Option<u64> {
    result.get("summary")?.get("errors")?.as_u64()
}

pub fn has_errors(result: &serde_json::Value) -> bool {
    error_count(result).is_some_and(|n| n > 0)
}

pub fn has_no_errors(result: &serde_json::Value) -> bool {
    error_count(result).is_some_and(|n| n == 0)
}

/// Clean up daemon socket artifacts for a test project's workspace root.
///
/// The actual daemon process is terminated by the workspace-root liveness check
/// (once the `TempDir` is dropped, the daemon detects the missing root and exits).
/// This helper removes leftover socket/lock/log files from the filesystem.
pub async fn cleanup_daemon(project: &impl TestProject) {
    use aether_lspd::{LanguageId, socket_path};

    for lang in [LanguageId::Rust, LanguageId::TypeScript] {
        let sock = socket_path(project.root(), lang);
        let _ = tokio::fs::remove_file(&sock).await;
        let _ = tokio::fs::remove_file(sock.with_extension("lock")).await;
        let _ = tokio::fs::remove_file(sock.with_extension("log")).await;
    }
}
