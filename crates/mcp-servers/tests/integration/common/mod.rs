//! Shared test helpers for MCP integration tests.

#![allow(dead_code)]

use aether_lspd::testing::{TestDaemon, TestDaemonBuilder};
use mcp_servers::coding::CodingMcp;
use mcp_servers::coding::tools::web_fetch::WebFetcher;
use mcp_servers::skills::{
    SkillsMcp,
    tools::{LoadSkillsInput, SkillRequest},
};
use mcp_servers::testing::{FakeHttpClient, TestWorkspace};
use mcp_utils::client::{ClientOptions, McpClient};
use mcp_utils::testing::{ElicitationScript, args, call, elicitation_host};
use rmcp::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, DetailedTask, ElicitResult, GetTaskParams, TaskPayload,
    TaskStatus,
};
use serde::Serialize;
use serde_json::{Value, json};
use std::fs::read_to_string;
use std::path::{Path, PathBuf};
use std::time::Duration;

const POLL_INTERVAL: Duration = Duration::from_millis(10);

pub(crate) type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

pub(crate) async fn connect(server: impl ServerHandler, options: ClientOptions) -> McpClient {
    mcp_utils::testing::connect("test", server, &options).await
}

pub(crate) fn cancels_prompts() -> ClientOptions {
    elicitation_host().0
}

/// A client that answers its first elicitation with `response`.
pub(crate) fn scripted(response: ElicitResult) -> (ClientOptions, ElicitationScript) {
    ElicitationScript::answering([response])
}

pub(crate) fn tool_request(tool: &str, arguments: impl Serialize) -> CallToolRequestParams {
    let arguments = serde_json::to_value(arguments).expect("tool arguments serialize to JSON");
    CallToolRequestParams::new(tool.to_string()).with_arguments(args(arguments))
}

pub(crate) fn taskless() -> ClientOptions {
    ClientOptions::default().without_tasks()
}

pub(crate) trait McpClientExt {
    async fn call_raw(&self, tool: &str, args: impl Serialize) -> TestResult<CallToolResult>;

    async fn call(&self, tool: &str, args: impl Serialize) -> TestResult<Value>;

    async fn call_displayed(&self, tool: &str, args: impl Serialize) -> TestResult<(Value, Value)>;
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

    async fn call_displayed(&self, tool: &str, args: impl Serialize) -> TestResult<(Value, Value)> {
        let result = self.call_raw(tool, args).await?;
        let meta = result.meta.map(|meta| Value::Object(meta.0)).unwrap_or_default();
        let output = result.structured_content.ok_or_else(|| test_error(format!("{tool} returned no output")))?;
        Ok((output, meta))
    }
}

pub(crate) fn test_error(message: impl Into<String>) -> std::io::Error {
    std::io::Error::other(message.into())
}

/// An isolated workspace connected to a `CodingMcp` through the public MCP protocol.
pub(crate) struct CodingWorkspace {
    pub client: McpClient,
    // Fields drop in order: stop the daemon before deleting the files it watches.
    daemon: Option<TestDaemon>,
    workspace: TestWorkspace,
}

impl CodingWorkspace {
    pub(crate) async fn new() -> Self {
        Self::with(TestWorkspace::new(), |root| CodingMcp::new(root)).await
    }

    pub(crate) async fn with_lsp(workspace: TestWorkspace) -> Self {
        Self::with(workspace, |root| CodingMcp::new(root).with_lsp()).await
    }

    pub(crate) async fn with_daemon(workspace: TestWorkspace, daemon: TestDaemonBuilder) -> Self {
        let daemon = daemon.spawn().expect("Failed to start LSP daemon");
        Self { daemon: Some(daemon), ..Self::with_lsp(workspace).await }
    }

    pub(crate) async fn with_http(http: FakeHttpClient) -> Self {
        Self::with(TestWorkspace::new(), |root| CodingMcp::new(root).with_web_fetcher(WebFetcher::with_client(http)))
            .await
    }

    pub(crate) async fn with(workspace: TestWorkspace, configure: impl FnOnce(&Path) -> CodingMcp) -> Self {
        let client = connect(configure(workspace.root()), ClientOptions::default()).await;
        Self { workspace, client, daemon: None }
    }

    pub(crate) fn root(&self) -> &Path {
        self.workspace.root()
    }

    pub(crate) fn path(&self, relative_path: impl AsRef<Path>) -> PathBuf {
        self.workspace.path(relative_path)
    }

    pub(crate) fn write(&self, relative_path: impl AsRef<Path>, content: &str) -> PathBuf {
        self.workspace.write(relative_path, content)
    }

    pub(crate) fn read(&self, relative_path: impl AsRef<Path>) -> TestResult<String> {
        Ok(read_to_string(self.path(relative_path))?)
    }

    pub(crate) async fn read_then_edit(&self, relative_path: &str, old: &str, new: &str) -> TestResult {
        let path = self.path(relative_path);
        self.client.call("read_file", json!({ "filePath": path })).await?;
        self.client
            .call("edit_file", json!({ "filePath": path, "edits": [{ "oldString": old, "newString": new }] }))
            .await?;
        Ok(())
    }

    pub(crate) async fn check_errors(&self, relative_path: &str) -> TestResult<Value> {
        self.client.call("lsp_check_errors", json!({ "filePath": self.path(relative_path) })).await
    }

    pub(crate) async fn poll_workspace_errors(&self, predicate: impl Fn(&Value) -> bool) -> Value {
        loop {
            if let Ok(result) = self.client.call("lsp_check_errors", json!({})).await
                && predicate(&result)
            {
                return result;
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

pub(crate) async fn call_tool_error(client: &McpClient, name: &str, args: Value) -> String {
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

pub(crate) async fn call_task(client: &McpClient, tool: &str, arguments: Value) -> TestResult<CallToolResult> {
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

pub(crate) async fn await_terminal_task(client: &McpClient, task_id: &str) -> TestResult<DetailedTask> {
    loop {
        let task = client.peer().get_task(GetTaskParams::new(task_id.to_string())).await?.task;
        if task.status().is_terminal() {
            return Ok(task);
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

fn error_count(result: &Value) -> Option<u64> {
    result.get("summary")?.get("errors")?.as_u64()
}

pub(crate) fn has_errors(result: &Value) -> bool {
    error_count(result).is_some_and(|n| n > 0)
}

pub(crate) fn has_no_errors(result: &Value) -> bool {
    error_count(result).is_some_and(|n| n == 0)
}

/// A skills server serving the `skills` directory of `test_dir`.
pub(crate) fn skills_server(test_dir: &Path) -> SkillsMcp {
    SkillsMcp::new(test_dir, &[test_dir.join("skills")])
}

pub(crate) fn load_skills_input(requests: &[(&str, Option<&str>)]) -> LoadSkillsInput {
    LoadSkillsInput {
        requests: requests
            .iter()
            .map(|(name, path)| SkillRequest { name: (*name).to_string(), path: path.map(str::to_string) })
            .collect(),
    }
}
