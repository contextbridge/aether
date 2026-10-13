use crate::common::{McpClientExt, TestResult, cancels_prompts, connect, scripted, tool_request};
use mcp_servers::PermissionMode;
use mcp_servers::coding::CodingMcp;
use mcp_utils::client::{ClientOptions, ElicitationSink};
use mcp_utils::model::{ElicitationCapability, UrlElicitationCapability};
use rmcp::model::{ElicitRequestParams, ElicitResult, ElicitationAction, ErrorCode, InputResponses};
use rmcp::service::ServiceError;
use serde_json::json;
use tempfile::TempDir;
use tokio::sync::mpsc;

fn coding_mcp(root: &TempDir, mode: PermissionMode) -> CodingMcp {
    CodingMcp::new(root.path()).with_permission_mode(mode)
}

fn allow() -> ElicitResult {
    ElicitResult::new(ElicitationAction::Accept).with_content(json!({ "decision": "allow" }))
}

fn deny() -> ElicitResult {
    ElicitResult::new(ElicitationAction::Accept).with_content(json!({ "decision": "deny" }))
}

#[tokio::test]
async fn always_ask_bash_runs_after_user_allows() -> TestResult {
    let root = TempDir::new()?;
    let (options, elicitation) = scripted(allow());
    let mcp = connect(coding_mcp(&root, PermissionMode::AlwaysAsk), options).await;

    let result = mcp.call("bash", json!({ "command": "echo hello" })).await?;
    assert!(result["output"].as_str().unwrap_or_default().contains("hello"));

    let captured = elicitation.captured().pop().expect("permission elicitation should be dispatched").request;
    let ElicitRequestParams::FormElicitationParams { message, requested_schema, .. } = captured else {
        panic!("expected form elicitation");
    };
    assert_eq!(message, "Allow bash: echo hello?");
    assert!(requested_schema.properties.contains_key("decision"));
    Ok(())
}

#[tokio::test]
async fn always_ask_bash_denied_returns_declined_error() -> TestResult {
    let root = TempDir::new()?;
    let (options, _elicitation) = scripted(deny());
    let mcp = connect(coding_mcp(&root, PermissionMode::AlwaysAsk), options).await;

    let result = mcp.call_raw("bash", json!({ "command": "echo hello" })).await?;
    assert!(result.is_error.unwrap_or(false), "denied command should error: {result:?}");
    let text = result.content.first().and_then(|c| c.as_text()).expect("error text");
    assert!(text.text.contains("Operation declined by user: bash"), "unexpected error: {}", text.text);
    Ok(())
}

#[tokio::test]
async fn auto_mode_safe_command_runs_without_elicitation() -> TestResult {
    let root = TempDir::new()?;
    // A silent client resolves any elicitation as Cancel, so success proves
    // no elicitation was dispatched.
    let mcp = connect(coding_mcp(&root, PermissionMode::Auto), cancels_prompts()).await;

    let result = mcp.call("bash", json!({ "command": "echo safe" })).await?;
    assert!(result["output"].as_str().unwrap_or_default().contains("safe"));
    Ok(())
}

#[tokio::test]
async fn auto_mode_dangerous_command_is_gated() -> TestResult {
    let root = TempDir::new()?;
    let (options, elicitation) = scripted(deny());
    let mcp = connect(coding_mcp(&root, PermissionMode::Auto), options).await;

    let result = mcp.call_raw("bash", json!({ "command": "rm -rf ./scratch" })).await?;
    assert!(result.is_error.unwrap_or(false), "denied dangerous command should error: {result:?}");

    let captured = elicitation.captured().pop().expect("dangerous command should trigger elicitation").request;
    assert!(matches!(
        captured,
        ElicitRequestParams::FormElicitationParams { message, .. } if message.contains("rm -rf ./scratch")
    ));
    Ok(())
}

#[tokio::test]
async fn always_ask_write_file_denied_leaves_file_unwritten() -> TestResult {
    let root = TempDir::new()?;
    let (options, _elicitation) = scripted(deny());
    let mcp = connect(coding_mcp(&root, PermissionMode::AlwaysAsk), options).await;

    let target = root.path().join("never.txt");
    let result =
        mcp.call_raw("write_file", json!({ "filePath": target.display().to_string(), "content": "nope" })).await?;
    assert!(result.is_error.unwrap_or(false), "denied write should error: {result:?}");
    assert!(!target.exists(), "denied write must not touch the filesystem");
    Ok(())
}

#[tokio::test]
async fn auto_mode_write_file_is_not_gated() -> TestResult {
    let root = TempDir::new()?;
    let mcp = connect(coding_mcp(&root, PermissionMode::Auto), cancels_prompts()).await;

    let target = root.path().join("free.txt");
    mcp.call("write_file", json!({ "filePath": target.display().to_string(), "content": "hi" })).await?;
    assert_eq!(std::fs::read_to_string(&target)?, "hi");
    Ok(())
}

#[tokio::test]
async fn cancelled_prompt_carrying_allow_content_is_not_approval() -> TestResult {
    let root = TempDir::new()?;
    let cancel_with_allow = ElicitResult::new(ElicitationAction::Cancel).with_content(json!({ "decision": "allow" }));
    let (options, _elicitation) = scripted(cancel_with_allow);
    let mcp = connect(coding_mcp(&root, PermissionMode::AlwaysAsk), options).await;

    let target = root.path().join("never.txt");
    let result = mcp.call_raw("bash", json!({ "command": format!("touch {}", target.display()) })).await?;
    assert!(result.is_error.unwrap_or(false), "cancelled approval should error: {result:?}");
    assert!(!target.exists(), "a cancelled prompt must not run the command");
    Ok(())
}

#[tokio::test]
async fn always_ask_gates_lsp_rename_via_destructive_annotation() -> TestResult {
    let root = TempDir::new()?;
    let (options, elicitation) = scripted(deny());
    let mcp = connect(coding_mcp(&root, PermissionMode::AlwaysAsk), options).await;

    let result = mcp.call_raw("lsp_rename", json!({ "symbol": "foo", "newName": "bar" })).await?;
    assert!(result.is_error.unwrap_or(false), "denied rename should error: {result:?}");
    let text = result.content.first().and_then(|c| c.as_text()).expect("error text");
    assert!(text.text.contains("Operation declined by user: lsp_rename"), "unexpected error: {}", text.text);

    let captured = elicitation.captured().pop().expect("destructive lsp_rename should be gated in AlwaysAsk").request;
    assert!(matches!(
        captured,
        ElicitRequestParams::FormElicitationParams { message, .. } if message.contains("lsp_rename")
    ));
    Ok(())
}

#[tokio::test]
async fn always_ask_without_elicitation_capability_denies_with_friendly_error() -> TestResult {
    let root = TempDir::new()?;
    let mcp = connect(coding_mcp(&root, PermissionMode::AlwaysAsk), ClientOptions::default()).await;

    let target = root.path().join("gated.txt");
    let result = mcp.call_raw("bash", json!({ "command": format!("touch {}", target.display()) })).await?;
    assert!(result.is_error.unwrap_or(false), "expected a tool error: {result:?}");
    let text = result.content.first().and_then(|c| c.as_text()).expect("error text");
    assert!(text.text.contains("cannot prompt"), "error should explain the capability gap: {}", text.text);
    assert!(!target.exists(), "the gated command must not run");
    Ok(())
}

#[tokio::test]
async fn clients_that_only_open_urls_cannot_answer_permission_prompts() -> TestResult {
    let root = TempDir::new()?;
    let (events, _host) = mpsc::channel(1);
    let url_only = ElicitationCapability::new().with_url(UrlElicitationCapability::new());
    let options = ClientOptions::default().elicitation(ElicitationSink::new(events, url_only));
    let mcp = connect(coding_mcp(&root, PermissionMode::AlwaysAsk), options).await;

    let result = mcp.call_raw("bash", json!({ "command": "echo hello" })).await?;

    let text = result.content.first().and_then(|c| c.as_text()).expect("error text");
    assert!(text.text.contains("cannot prompt"), "url-only clients cannot fill in forms: {}", text.text);
    Ok(())
}

#[tokio::test]
async fn permission_answers_must_include_the_permission_decision() -> TestResult {
    let root = TempDir::new()?;
    let mcp = connect(coding_mcp(&root, PermissionMode::AlwaysAsk), cancels_prompts()).await;

    let missing = answer_permission(&mcp, InputResponses::new()).await;
    let malformed = answer_permission(&mcp, InputResponses::from([("permission".to_string(), json!("yes"))])).await;

    assert_invalid_params(missing, "missing input response for 'permission'");
    assert_invalid_params(malformed, "invalid input response for 'permission'");
    Ok(())
}

async fn answer_permission(
    mcp: &mcp_utils::client::McpClient,
    responses: InputResponses,
) -> Result<rmcp::model::CallToolResponse, ServiceError> {
    let mut request = tool_request("bash", json!({ "command": "echo hello" }));
    request.input_responses = Some(responses);
    mcp.peer().call_tool_once(request).await
}

fn assert_invalid_params(result: Result<rmcp::model::CallToolResponse, ServiceError>, message: &str) {
    match result {
        Err(ServiceError::McpError(error)) => {
            assert_eq!(error.code, ErrorCode::INVALID_PARAMS);
            assert!(error.message.contains(message), "unexpected error: {}", error.message);
        }
        other => panic!("expected an invalid params error, got {other:?}"),
    }
}
