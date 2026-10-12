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
use mcp_utils::client::{ClientOptions, McpClient};
use mcp_utils::testing::{ElicitationScript, args, call, elicitation_host};
use rmcp::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, DetailedTask, ElicitResult, GetTaskParams, TaskPayload,
    TaskStatus,
};
use serde::Serialize;
use serde_json::Value;
use std::fs::{create_dir_all, read_to_string};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tempfile::{TempDir, tempdir};

/// Default timeout for polling operations (60 seconds).
const POLL_TIMEOUT: Duration = Duration::from_mins(1);
const POLL_INTERVAL: Duration = Duration::from_millis(500);
const TASK_POLL_INTERVAL: Duration = Duration::from_millis(10);

pub type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

pub async fn connect(server: impl ServerHandler, options: ClientOptions) -> McpClient {
    mcp_utils::testing::connect("test", server, &options).await
}

pub fn cancels_prompts() -> ClientOptions {
    elicitation_host().0
}

/// A client that answers its first elicitation with `response`.
pub fn scripted(response: ElicitResult) -> (ClientOptions, ElicitationScript) {
    ElicitationScript::answering([response])
}

pub fn tool_request(tool: &str, arguments: impl Serialize) -> CallToolRequestParams {
    let arguments = serde_json::to_value(arguments).expect("tool arguments serialize to JSON");
    CallToolRequestParams::new(tool.to_string()).with_arguments(args(arguments))
}

pub fn taskless() -> ClientOptions {
    ClientOptions::default().without_tasks()
}

pub fn coding_with_lsp(project: &impl TestProject) -> CodingMcp {
    CodingMcp::new(project.root()).with_lsp()
}

pub trait McpClientExt {
    async fn call_raw(&self, tool: &str, args: impl Serialize) -> TestResult<CallToolResult>;

    async fn call(&self, tool: &str, args: impl Serialize) -> TestResult<Value>;
}

impl McpClientExt for McpClient {
    async fn call_raw(&self, tool: &str, args: impl Serialize) -> TestResult<CallToolResult> {
        Ok(call(self, tool, serde_json::to_value(args)?).await?)
    }

    async fn call(&self, tool: &str, args: impl Serialize) -> TestResult<Value> {
        let result = self.call_raw(tool, args).await?;
        let text = result
            .content
            .first()
            .and_then(|content| content.as_text())
            .ok_or_else(|| test_error(format!("{tool} returned no text content: {result:?}")))?;
        serde_json::from_str(&text.text)
            .map_err(|error| test_error(format!("{tool} returned non-JSON text ({error}): {}", text.text)).into())
    }
}

pub fn test_error(message: impl Into<String>) -> std::io::Error {
    std::io::Error::other(message.into())
}

/// An isolated workspace connected to a `CodingMcp` through the public MCP protocol.
pub struct CodingWorkspace {
    root: TempDir,
    pub client: McpClient,
}

impl CodingWorkspace {
    pub async fn new() -> TestResult<Self> {
        Self::start(|root| CodingMcp::new(root)).await
    }

    pub async fn new_with_lsp() -> TestResult<Self> {
        Self::start(|root| CodingMcp::new(root).with_lsp()).await
    }

    pub async fn with_http(http: FakeHttpClient) -> TestResult<Self> {
        Self::start(|root| CodingMcp::new(root).with_web_fetcher(WebFetcher::with_client(http))).await
    }

    async fn start(configure: impl FnOnce(&Path) -> CodingMcp) -> TestResult<Self> {
        let root = tempdir()?;
        let client = connect(configure(root.path()), ClientOptions::default()).await;
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

pub async fn call_tool_error(client: &McpClient, name: &str, args: Value) -> String {
    match client.call_raw(name, args).await {
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
    client.call(name, args).await.unwrap_or_else(|error| panic!("Tool '{name}' did not return valid JSON: {error}"))
}

pub async fn try_call_tool(client: &McpClient, name: &str, args: Value) -> Option<Value> {
    client.call(name, args).await.inspect_err(|error| eprintln!("[try_call_tool] {name}: {error}")).ok()
}

pub async fn call_task(client: &McpClient, tool: &str, arguments: Value) -> TestResult<CallToolResult> {
    let response = client.peer().call_tool_once(tool_request(tool, arguments)).await?;
    let CallToolResponse::Task(created) = response else {
        return Err(test_error(format!("expected task response: {response:?}")).into());
    };
    assert_eq!(created.task.status, TaskStatus::Working);

    let task = await_terminal_task(client, &created.task.task_id).await?;
    let TaskPayload::Completed { result } = task.payload else {
        return Err(test_error(format!("expected completed task: {task:?}")).into());
    };
    Ok(serde_json::from_value(Value::Object(result))?)
}

pub async fn await_terminal_task(client: &McpClient, task_id: &str) -> TestResult<DetailedTask> {
    loop {
        let task = client.peer().get_task(GetTaskParams::new(task_id.to_string())).await?.task;
        if task.status().is_terminal() {
            return Ok(task);
        }
        tokio::time::sleep(TASK_POLL_INTERVAL).await;
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
    SkillsMcp::new(test_dir, &[test_dir.join("skills")])
}

pub fn load_skills_input(requests: &[(&str, Option<&str>)]) -> LoadSkillsInput {
    LoadSkillsInput {
        requests: requests
            .iter()
            .map(|(name, path)| SkillRequest { name: (*name).to_string(), path: path.map(str::to_string) })
            .collect(),
    }
}
