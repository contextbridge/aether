use mcp_servers::CodingMcp;
use mcp_servers::coding::error::BashError;
use mcp_servers::coding::tools::bash::{BashInput, execute_command, execute_command_in_dir};
use rmcp::model::{CallToolRequestParams, CallToolResponse, TaskPayload, TaskStatus};
use std::fs::canonicalize;

use super::common::{TestClient, TestResult, production_client_info, test_client_info};

#[tokio::test]
async fn test_basic_command() {
    let output =
        execute_command(BashInput { command: "echo 'hello world'".into(), ..Default::default() }).await.unwrap();
    assert_eq!(output.output.trim(), "hello world");
    assert_eq!(output.exit_code, 0);
    assert!(!output.killed);
}

#[tokio::test]
async fn test_command_with_exit_code_and_stderr() {
    let output =
        execute_command(BashInput { command: "echo error >&2; exit 42".into(), ..Default::default() }).await.unwrap();
    assert!(output.output.contains("error"));
    assert_eq!(output.exit_code, 42);
    assert!(!output.killed);
}

#[tokio::test]
async fn test_command_timeout() {
    let output = execute_command(BashInput { command: "sleep 10".into(), timeout: Some(100), ..Default::default() })
        .await
        .unwrap();
    assert!(output.output.contains("timed out"));
    assert_eq!(output.exit_code, -1);
    assert!(output.killed);
}

#[tokio::test]
async fn test_timeout_validation() {
    let result =
        execute_command(BashInput { command: "echo test".into(), timeout: Some(700_000), ..Default::default() }).await;
    assert!(matches!(result.unwrap_err(), BashError::TimeoutTooLarge));
}

#[tokio::test]
async fn test_rm_command_blocked() {
    let result = execute_command(BashInput { command: "rm".into(), ..Default::default() }).await;
    assert!(matches!(result.unwrap_err(), BashError::Forbidden(_)));
}

#[tokio::test]
async fn test_execute_command_in_dir_foreground() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let output =
        execute_command_in_dir(BashInput { command: "pwd".into(), ..Default::default() }, Some(temp.path())).await?;
    assert_eq!(canonicalize(output.output.trim())?, canonicalize(temp.path())?);
    Ok(())
}

#[tokio::test]
async fn background_bash_uses_tasks_and_returns_structured_output() -> TestResult {
    let client = TestClient::start_with(CodingMcp::new, production_client_info()).await?;
    let response = client
        .raw()
        .call_tool_once(
            CallToolRequestParams::new("bash").with_arguments(
                serde_json::json!({
                    "command": "echo stdout; echo stderr >&2; exit 7",
                    "runInBackground": true
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .await?;
    let CallToolResponse::Task(created) = response else {
        panic!("expected task response: {response:?}");
    };
    assert_eq!(created.task.status, TaskStatus::Working);

    let detailed = loop {
        let result = client.raw().get_task(rmcp::model::GetTaskParams::new(created.task.task_id.clone())).await?;
        if result.task.status().is_terminal() {
            break result.task;
        }
        tokio::task::yield_now().await;
    };
    let TaskPayload::Completed { result } = detailed.payload else {
        panic!("expected completed task");
    };
    let result: rmcp::model::CallToolResult = serde_json::from_value(serde_json::Value::Object(result))?;
    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.as_ref().unwrap();
    let output = structured["output"].as_str().unwrap();
    assert!(output.contains("stdout"));
    assert!(output.contains("stderr"));
    assert_eq!(structured["exitCode"], 7);
    Ok(())
}

#[tokio::test]
async fn background_bash_requires_tasks_capability() -> TestResult {
    let client = TestClient::start_with(CodingMcp::new, test_client_info()).await?;
    let error = client
        .raw()
        .call_tool_once(CallToolRequestParams::new("bash").with_arguments(
            serde_json::json!({ "command": "echo never", "runInBackground": true }).as_object().unwrap().clone(),
        ))
        .await
        .unwrap_err();
    assert!(
        error.to_string().to_lowercase().contains("missing required client capability"),
        "unexpected error: {error}"
    );
    Ok(())
}
