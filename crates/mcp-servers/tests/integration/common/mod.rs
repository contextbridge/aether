//! Shared test helpers for MCP integration tests.

#![allow(dead_code)]

use aether_lspd::testing::TestProject;
use mcp_servers::coding::CodingMcp;
use mcp_servers::coding::tools::web_fetch::WebFetcher;
use mcp_servers::skills::{
    SkillsMcp,
    tools::{LoadSkillsInput, SkillRequest},
};
use mcp_servers::testing::FakeHttpClient;
use mcp_utils::client::{ClientOptions, McpClient, ToolCallOptions};
use mcp_utils::server::McpServer;
use mcp_utils::testing::{ElicitationScript, args, connect};
use rmcp::ServerHandler;
use rmcp::model::{CallToolResult, ClientCapabilities, ClientConfig, ElicitResult, Implementation};
use serde::Serialize;
use serde_json::Value;
use std::fs::{create_dir_all, read_to_string};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tempfile::{TempDir, tempdir};
use tokio::sync::mpsc;

/// Default timeout for polling operations (60 seconds).
const POLL_TIMEOUT: Duration = Duration::from_mins(1);
const POLL_INTERVAL: Duration = Duration::from_millis(500);

pub type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

pub fn test_client_info() -> ClientConfig {
    ClientConfig::new(ClientCapabilities::default(), Implementation::new("test-client", "0.1.0"))
}

pub fn production_client_info() -> ClientConfig {
    silent_client().client_config()
}

pub fn silent_client() -> ClientOptions {
    let (event_tx, _event_rx) = mpsc::channel(8);
    ClientOptions::default().elicitation(event_tx)
}

pub fn scripted_client(response: ElicitResult) -> (ClientOptions, ElicitationScript) {
    let (event_tx, event_rx) = mpsc::channel(8);
    (ClientOptions::default().elicitation(event_tx), ElicitationScript::spawn(event_rx, [response]))
}

pub fn test_error(message: impl Into<String>) -> std::io::Error {
    std::io::Error::other(message.into())
}

/// An isolated workspace connected to a `CodingMcp` through the public MCP protocol.
pub struct CodingWorkspace {
    root: TempDir,
    pub client: TestClient,
}

impl CodingWorkspace {
    pub async fn new() -> TestResult<Self> {
        Self::start(|root| CodingMcp::new().with_root_dir(root.to_path_buf())).await
    }

    pub async fn new_with_lsp() -> TestResult<Self> {
        Self::start(|root| CodingMcp::new().with_lsp(root.to_path_buf())).await
    }

    pub async fn with_http(http: FakeHttpClient) -> TestResult<Self> {
        Self::start(|root| {
            CodingMcp::new().with_root_dir(root.to_path_buf()).with_web_fetcher(WebFetcher::with_client(http))
        })
        .await
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

pub struct TestClient {
    client: McpClient,
}

impl TestClient {
    pub async fn start<T: ServerHandler>(configure: impl FnOnce() -> T) -> TestResult<Self> {
        Self::start_with(configure, ClientOptions::default()).await
    }

    pub async fn start_with<T: ServerHandler>(
        configure: impl FnOnce() -> T,
        options: ClientOptions,
    ) -> TestResult<Self> {
        Ok(Self { client: connect("test-server", McpServer::new(configure()), &options).await })
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
        Ok(call(&self.client, tool, serde_json::to_value(args)?).await?)
    }

    pub fn mcp(&self) -> &McpClient {
        &self.client
    }
}

pub async fn connect_lsp(project: &impl TestProject) -> McpClient {
    connect_coding(CodingMcp::new().with_lsp(project.root().to_path_buf())).await
}

pub async fn connect_coding<T: ServerHandler>(server: T) -> McpClient {
    connect("coding", McpServer::new(server), &ClientOptions::default()).await
}

pub async fn call_tool_error(client: &McpClient, name: &str, args: Value) -> String {
    match call(client, name, args).await {
        Ok(result) => {
            assert!(result.is_error.unwrap_or(false), "tool call should fail: {result:?}");
            let content = result.content.first().expect("Expected error content");
            let text = content.as_text().expect("Expected text error content");
            text.text.clone()
        }
        Err(error) => error.to_string(),
    }
}

pub async fn call_tool(client: &McpClient, name: &str, args: Value) -> Value {
    try_call_tool(client, name, args).await.unwrap_or_else(|| panic!("Tool '{name}' did not return valid JSON"))
}

pub async fn try_call_tool(client: &McpClient, name: &str, args: Value) -> Option<Value> {
    let result = match call(client, name, args).await {
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
    client: &McpClient,
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
    client: &McpClient,
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

async fn call(
    client: &McpClient,
    tool: &str,
    arguments: Value,
) -> Result<CallToolResult, mcp_utils::client::ToolCallError> {
    client.call_tool(tool, args(arguments), ToolCallOptions::default()).result().await
}

/// Creates files and directories (including parents) from `(path, content)`
/// pairs inside a fresh temp dir.
pub fn create_test_files(files: &[(&str, &str)]) -> TempDir {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    for (path, content) in files {
        let full_path = temp_dir.path().join(path);
        if let Some(parent) = full_path.parent() {
            create_dir_all(parent).unwrap_or_else(|_| panic!("Failed to create directory for {path}"));
        }
        std::fs::write(&full_path, content).unwrap_or_else(|_| panic!("Failed to write file {path}"));
    }
    temp_dir
}

/// A skills server serving the `skills` directory of `test_dir`.
pub fn skills_server(test_dir: &Path) -> SkillsMcp {
    SkillsMcp::new(&[test_dir.join("skills")])
}

pub fn load_skills_input(requests: &[(&str, Option<&str>)]) -> LoadSkillsInput {
    LoadSkillsInput {
        requests: requests
            .iter()
            .map(|(name, path)| SkillRequest { name: (*name).to_string(), path: path.map(str::to_string) })
            .collect(),
    }
}
