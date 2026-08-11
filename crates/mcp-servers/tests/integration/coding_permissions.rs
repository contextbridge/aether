use crate::common::{TestClient, TestResult, scripted_mcp_client, silent_mcp_client, test_client_info};
use mcp_servers::coding::CodingMcp;
use mcp_servers::{DefaultCodingTools, PermissionMode};
use rmcp::model::{ElicitRequestParams, ElicitResult, ElicitationAction};
use serde_json::json;
use tempfile::TempDir;

fn coding_mcp(root: &TempDir, mode: PermissionMode) -> CodingMcp<DefaultCodingTools> {
    CodingMcp::new().with_root_dir(root.path().to_path_buf()).with_permission_mode(mode)
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
    let (client, elicitation) = scripted_mcp_client("coding", allow());
    let mcp = TestClient::start_with(|| coding_mcp(&root, PermissionMode::AlwaysAsk), client).await?;

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
    let (client, _elicitation) = scripted_mcp_client("coding", deny());
    let mcp = TestClient::start_with(|| coding_mcp(&root, PermissionMode::AlwaysAsk), client).await?;

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
    let mcp = TestClient::start_with(|| coding_mcp(&root, PermissionMode::Auto), silent_mcp_client("coding")).await?;

    let result = mcp.call("bash", json!({ "command": "echo safe" })).await?;
    assert!(result["output"].as_str().unwrap_or_default().contains("safe"));
    Ok(())
}

#[tokio::test]
async fn auto_mode_dangerous_command_is_gated() -> TestResult {
    let root = TempDir::new()?;
    let (client, elicitation) = scripted_mcp_client("coding", deny());
    let mcp = TestClient::start_with(|| coding_mcp(&root, PermissionMode::Auto), client).await?;

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
    let (client, _elicitation) = scripted_mcp_client("coding", deny());
    let mcp = TestClient::start_with(|| coding_mcp(&root, PermissionMode::AlwaysAsk), client).await?;

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
    let mcp = TestClient::start_with(|| coding_mcp(&root, PermissionMode::Auto), silent_mcp_client("coding")).await?;

    let target = root.path().join("free.txt");
    mcp.call("write_file", json!({ "filePath": target.display().to_string(), "content": "hi" })).await?;
    assert_eq!(std::fs::read_to_string(&target)?, "hi");
    Ok(())
}

#[tokio::test]
async fn cancelled_prompt_carrying_allow_content_is_not_approval() -> TestResult {
    let root = TempDir::new()?;
    let cancel_with_allow = ElicitResult::new(ElicitationAction::Cancel).with_content(json!({ "decision": "allow" }));
    let (client, _elicitation) = scripted_mcp_client("coding", cancel_with_allow);
    let mcp = TestClient::start_with(|| coding_mcp(&root, PermissionMode::AlwaysAsk), client).await?;

    let target = root.path().join("never.txt");
    let result = mcp.call_raw("bash", json!({ "command": format!("touch {}", target.display()) })).await?;
    assert!(result.is_error.unwrap_or(false), "cancelled approval should error: {result:?}");
    assert!(!target.exists(), "a cancelled prompt must not run the command");
    Ok(())
}

#[tokio::test]
async fn always_ask_gates_lsp_rename_via_destructive_annotation() -> TestResult {
    let root = TempDir::new()?;
    let (client, elicitation) = scripted_mcp_client("coding", deny());
    let mcp = TestClient::start_with(|| coding_mcp(&root, PermissionMode::AlwaysAsk), client).await?;

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
    let mcp = TestClient::start_with(|| coding_mcp(&root, PermissionMode::AlwaysAsk), test_client_info()).await?;

    let target = root.path().join("gated.txt");
    let result = mcp.call_raw("bash", json!({ "command": format!("touch {}", target.display()) })).await?;
    assert!(result.is_error.unwrap_or(false), "expected a tool error: {result:?}");
    let text = result.content.first().and_then(|c| c.as_text()).expect("error text");
    assert!(text.text.contains("cannot prompt"), "error should explain the capability gap: {}", text.text);
    assert!(!target.exists(), "the gated command must not run");
    Ok(())
}
