//! Shared test helpers for MCP integration tests.

#![allow(dead_code)]

use aether_lspd::testing::TestProject;
use mcp_servers::coding::CodingMcp;
use mcp_utils::testing::connect;
use rmcp::RoleClient;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ClientCapabilities, ClientInfo, FormElicitationCapability, Implementation,
    UrlElicitationCapability,
};
use rmcp::service::RunningService;
use rmcp::{RoleServer, Service};
use serde::Serialize;
use std::fs::{create_dir_all, read_to_string};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tempfile::{TempDir, tempdir};

/// Default timeout for polling operations (60 seconds).
const POLL_TIMEOUT: Duration = Duration::from_mins(1);
const POLL_INTERVAL: Duration = Duration::from_millis(500);

pub type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

pub fn test_client_info() -> ClientInfo {
    let mut capabilities = ClientCapabilities::builder().enable_elicitation().build();
    if let Some(elicitation) = capabilities.elicitation.as_mut() {
        elicitation.form = Some(FormElicitationCapability::default());
        elicitation.url = Some(UrlElicitationCapability::default());
    }
    ClientInfo::new(capabilities, Implementation::new("test-client", "0.1.0"))
}

pub fn test_error(message: impl Into<String>) -> std::io::Error {
    std::io::Error::other(message.into())
}

/// An isolated workspace connected to a `CodingMcp` through the public MCP protocol.
pub struct CodingWorkspace {
    root: TempDir,
    pub client: TestClient<CodingMcp>,
}

impl CodingWorkspace {
    pub async fn new() -> TestResult<Self> {
        Self::start(|root| CodingMcp::new().with_root_dir(root.to_path_buf())).await
    }

    pub async fn new_with_lsp() -> TestResult<Self> {
        Self::start(|root| CodingMcp::new().with_lsp(root.to_path_buf())).await
    }

    async fn start(configure: impl FnOnce(&Path) -> CodingMcp) -> TestResult<Self> {
        let root = tempdir()?;
        let client = TestClient::start(|| configure(root.path())).await?;
        Ok(Self { root, client })
    }

    pub fn root(&self) -> &Path {
        self.root.path()
    }

    pub fn path(&self, relative_path: impl AsRef<Path>) -> PathBuf {
        self.root.path().join(relative_path)
    }

    pub fn write(&self, relative_path: impl AsRef<Path>, content: &str) -> TestResult<PathBuf> {
        let path = self.path(relative_path);
        if let Some(parent) = path.parent() {
            create_dir_all(parent)?;
        }
        std::fs::write(&path, content)?;
        Ok(path)
    }

    pub fn read(&self, relative_path: impl AsRef<Path>) -> TestResult<String> {
        Ok(read_to_string(self.path(relative_path))?)
    }
}

/// Generic test client wrapping a connected MCP server.
pub struct TestClient<T: Service<RoleServer>, U: Service<RoleClient> = ClientInfo> {
    _server_handle: RunningService<RoleServer, T>,
    client: RunningService<RoleClient, U>,
}

impl<T: Service<RoleServer>> TestClient<T, ClientInfo> {
    /// Connect a default test `ClientInfo` to a server built by `configure`.
    pub async fn start(configure: impl FnOnce() -> T) -> TestResult<Self> {
        let (server_handle, client) = connect(configure(), test_client_info()).await?;
        Ok(Self { _server_handle: server_handle, client })
    }
}

impl<T: Service<RoleServer>, U: Service<RoleClient>> TestClient<T, U> {
    pub async fn start_with(configure_server: impl FnOnce() -> T, client: U) -> TestResult<Self> {
        let (server_handle, client) = connect(configure_server(), client).await?;
        Ok(Self { _server_handle: server_handle, client })
    }

    pub async fn call<V: Serialize>(&self, tool: &str, args: V) -> TestResult<serde_json::Value> {
        let result = self.call_raw(tool, args).await?;
        let text = result
            .content
            .first()
            .and_then(|c| c.as_text())
            .ok_or_else(|| test_error(format!("{tool} should return text content")))?;
        Ok(serde_json::from_str(&text.text)?)
    }

    pub async fn call_raw<V: Serialize>(&self, tool: &str, args: V) -> TestResult<CallToolResult> {
        let args = serde_json::to_value(args)?;
        let arguments =
            args.as_object().ok_or_else(|| test_error("tool arguments must serialize to a JSON object"))?.clone();
        Ok(self.client.call_tool(CallToolRequestParams::new(tool.to_string()).with_arguments(arguments)).await?)
    }

    pub fn raw(&self) -> &RunningService<RoleClient, U> {
        &self.client
    }
}

pub async fn connect_lsp(
    project: &impl TestProject,
) -> (RunningService<RoleServer, CodingMcp>, RunningService<RoleClient, ClientInfo>) {
    let server = CodingMcp::new().with_lsp(project.root().to_path_buf());
    connect(server, test_client_info()).await.expect("Failed to connect")
}

pub async fn call_tool_error(
    client: &RunningService<RoleClient, ClientInfo>,
    name: &str,
    args: serde_json::Value,
) -> String {
    match client
        .call_tool(CallToolRequestParams::new(name.to_string()).with_arguments(args.as_object().unwrap().clone()))
        .await
    {
        Ok(result) => {
            assert!(result.is_error.unwrap_or(false), "tool call should fail: {result:?}");
            let content = result.content.first().expect("Expected error content");
            let text = content.as_text().expect("Expected text error content");
            text.text.clone()
        }
        Err(error) => error.to_string(),
    }
}

pub async fn call_tool(
    client: &RunningService<RoleClient, ClientInfo>,
    name: &str,
    args: serde_json::Value,
) -> serde_json::Value {
    try_call_tool(client, name, args).await.unwrap_or_else(|| panic!("Tool '{name}' did not return valid JSON"))
}

pub async fn try_call_tool(
    client: &RunningService<RoleClient, ClientInfo>,
    name: &str,
    args: serde_json::Value,
) -> Option<serde_json::Value> {
    let result = match client
        .call_tool(CallToolRequestParams::new(name.to_string()).with_arguments(args.as_object().unwrap().clone()))
        .await
    {
        Ok(result) => result,
        Err(error) => {
            eprintln!("[try_call_tool] {name} RPC error: {error}");
            return None;
        }
    };
    let Some(text) = result.content.first().and_then(|c| c.as_text()) else {
        eprintln!("[try_call_tool] {name} no text content in response");
        return None;
    };
    if let Ok(value) = serde_json::from_str(&text.text) {
        Some(value)
    } else {
        eprintln!("[try_call_tool] {name} non-JSON response: {}", text.text);
        None
    }
}

pub async fn poll_diagnostics(
    client: &RunningService<RoleClient, ClientInfo>,
    file_path: Option<&str>,
    predicate: impl Fn(&serde_json::Value) -> bool,
) -> serde_json::Value {
    let args = match file_path {
        Some(path) => serde_json::json!({ "filePath": path }),
        None => serde_json::json!({}),
    };
    poll_lsp_tool(client, "lsp_check_errors", args, predicate).await
}

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

pub async fn cleanup_daemon(project: &impl TestProject) {
    use aether_lspd::{LanguageId, socket_path};
    for lang in [LanguageId::Rust, LanguageId::TypeScript] {
        let sock = socket_path(project.root(), lang);
        let _ = tokio::fs::remove_file(&sock).await;
        let _ = tokio::fs::remove_file(sock.with_extension("lock")).await;
        let _ = tokio::fs::remove_file(sock.with_extension("log")).await;
    }
}
