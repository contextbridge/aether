//! Public server integration tests for the Phase 2.2 MRTR conversion of the
//! built-in servers. Each test drives the real server through the public MCP
//! protocol (in-memory transport, modern lifecycle) with a real `McpClient`
//! handler, and answers the server's `InputRequiredResult` rounds through the
//! elicitation event channel.
//!
//! Covered here: coding permission, submit-plan review, and survey submission,
//! including accept, decline, cancel, and additional-round behavior.

use crate::common::{TestClient, TestResult, test_client_info};
use mcp_servers::coding::CodingMcp;
use mcp_servers::plan::{PlanMcp, SubmitPlanInput};
use mcp_servers::survey::SurveyMcp;
use mcp_utils::client::{McpClient, McpClientEvent};
use rmcp::model::{CallToolRequestParams, CallToolResponse, ElicitRequestParams, ElicitResult, ElicitationAction};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::sync::mpsc;

/// Script the elicitation channel: capture the first elicitation request and
/// answer it with `response`.
fn respond_to_first_elicitation(
    mut event_rx: mpsc::Receiver<McpClientEvent>,
    response: ElicitResult,
) -> tokio::task::JoinHandle<Option<ElicitRequestParams>> {
    tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            if let McpClientEvent::Elicitation(req) = event {
                let captured = req.request;
                let _ = req.response_sender.send(response);
                return Some(captured);
            }
        }
        None
    })
}

fn elicitation_client(server_name: &str) -> (McpClient, mpsc::Receiver<McpClientEvent>) {
    let (event_tx, event_rx) = mpsc::channel(8);
    (McpClient::new(test_client_info(), server_name.to_string(), event_tx), event_rx)
}

// Coding permission

async fn coding_with_permission(
    mode: mcp_servers::coding::PermissionMode,
    root: &TempDir,
    client: McpClient,
) -> TestResult<TestClient<CodingMcp, McpClient>> {
    let server = CodingMcp::new().with_root_dir(root.path().to_path_buf()).with_permission_mode(mode);
    Box::pin(TestClient::start_with(|| server, client)).await
}

#[tokio::test]
#[allow(clippy::large_futures)]
async fn coding_bash_always_ask_allow_runs_command() -> TestResult {
    let root = TempDir::new()?;
    let (client, event_rx) = elicitation_client("coding-test");
    let script = respond_to_first_elicitation(
        event_rx,
        ElicitResult::new(ElicitationAction::Accept).with_content(json!({ "decision": "allow" })),
    );
    let mcp = coding_with_permission(mcp_servers::coding::PermissionMode::AlwaysAsk, &root, client).await?;

    let result = mcp.call("bash", json!({ "command": "echo mrtr-ok" })).await?;
    assert!(result["output"].as_str().unwrap_or_default().contains("mrtr-ok"), "bash should have run: {result}");

    let request = script.await?.expect("permission elicitation should be dispatched");
    let ElicitRequestParams::FormElicitationParams { message, requested_schema, .. } = request else {
        panic!("permission check should be a form elicitation");
    };
    assert!(message.contains("bash"), "message should name the tool: {message}");
    assert!(requested_schema.properties.contains_key("decision"), "form should ask for a decision");
    Ok(())
}

#[tokio::test]
#[allow(clippy::large_futures)]
async fn coding_bash_always_ask_deny_returns_tool_error() -> TestResult {
    let root = TempDir::new()?;
    let (client, event_rx) = elicitation_client("coding-test");
    let script = respond_to_first_elicitation(
        event_rx,
        ElicitResult::new(ElicitationAction::Accept).with_content(json!({ "decision": "deny" })),
    );
    let mcp = coding_with_permission(mcp_servers::coding::PermissionMode::AlwaysAsk, &root, client).await?;

    let result = mcp.call_raw("bash", json!({ "command": "echo should-not-run" })).await?;
    assert!(result.is_error.unwrap_or(false), "denied bash should be a tool error: {result:?}");
    let text = result.content.first().and_then(|c| c.as_text()).map(|t| t.text.clone()).unwrap_or_default();
    assert!(text.contains("declined"), "error should mention decline: {text}");
    assert!(script.await?.is_some());
    Ok(())
}

#[tokio::test]
#[allow(clippy::large_futures)]
async fn coding_bash_always_ask_decline_and_cancel_return_tool_errors() -> TestResult {
    for (action, expected) in [(ElicitationAction::Decline, "declined"), (ElicitationAction::Cancel, "cancelled")] {
        let root = TempDir::new()?;
        let (client, event_rx) = elicitation_client("coding-test");
        let script = respond_to_first_elicitation(event_rx, ElicitResult::new(action.clone()));
        let mcp = coding_with_permission(mcp_servers::coding::PermissionMode::AlwaysAsk, &root, client).await?;

        let result = mcp.call_raw("bash", json!({ "command": "echo should-not-run" })).await?;
        assert!(result.is_error.unwrap_or(false), "{action:?} bash should be a tool error: {result:?}");
        let text = result.content.first().and_then(|c| c.as_text()).map(|t| t.text.clone()).unwrap_or_default();
        assert!(text.contains(expected), "error should mention {expected}: {text}");
        assert!(script.await?.is_some());
    }
    Ok(())
}

#[tokio::test]
#[allow(clippy::large_futures)]
async fn coding_bash_auto_mode_only_elicits_dangerous_commands() -> TestResult {
    let root = TempDir::new()?;
    let (client, event_rx) = elicitation_client("coding-test");
    let script = respond_to_first_elicitation(
        event_rx,
        ElicitResult::new(ElicitationAction::Accept).with_content(json!({ "decision": "allow" })),
    );
    let mcp = coding_with_permission(mcp_servers::coding::PermissionMode::Auto, &root, client).await?;

    // Safe command: no elicitation, runs directly.
    let safe = mcp.call("bash", json!({ "command": "echo safe" })).await?;
    assert!(safe["output"].as_str().unwrap_or_default().contains("safe"));

    // Dangerous command: elicitation round opens, then runs after approval.
    let doomed = root.path().join("doomed.txt");
    std::fs::write(&doomed, "delete me")?;
    let dangerous = mcp.call("bash", json!({ "command": format!("rm -rf {}", doomed.display()) })).await?;
    assert_eq!(dangerous["exitCode"], 0, "approved rm should succeed: {dangerous}");
    assert!(!doomed.exists(), "approved rm should have deleted the file");
    let request = script.await?.expect("dangerous command should elicit");
    assert!(matches!(request, ElicitRequestParams::FormElicitationParams { .. }));
    Ok(())
}

#[tokio::test]
#[allow(clippy::large_futures)]
async fn coding_write_file_always_ask_elicits_before_writing() -> TestResult {
    let root = TempDir::new()?;
    let (client, event_rx) = elicitation_client("coding-test");
    let script = respond_to_first_elicitation(
        event_rx,
        ElicitResult::new(ElicitationAction::Accept).with_content(json!({ "decision": "allow" })),
    );
    let mcp = coding_with_permission(mcp_servers::coding::PermissionMode::AlwaysAsk, &root, client).await?;

    let result = mcp.call("write_file", json!({ "filePath": "hello.txt", "content": "hi" })).await?;
    assert!(result["message"].as_str().is_some(), "write should succeed: {result}");
    assert_eq!(std::fs::read_to_string(root.path().join("hello.txt"))?, "hi");

    let request = script.await?.expect("write should elicit");
    let ElicitRequestParams::FormElicitationParams { message, .. } = request else {
        panic!("write permission should be a form elicitation");
    };
    assert!(message.contains("write_file"), "message should name the tool: {message}");
    Ok(())
}

#[tokio::test]
#[allow(clippy::large_futures)]
async fn coding_permission_retry_with_unknown_state_fails() -> TestResult {
    let root = TempDir::new()?;
    let (client, _event_rx) = elicitation_client("coding-test");
    let mcp = coding_with_permission(mcp_servers::coding::PermissionMode::AlwaysAsk, &root, client).await?;

    // First call opens a round and parks a token; a forged retry cannot pass.
    let forged = mcp
        .raw()
        .call_tool(
            rmcp::model::CallToolRequestParams::new("bash")
                .with_arguments(json!({ "command": "echo x" }).as_object().unwrap().clone())
                .with_input_responses(rmcp::model::InputResponses::from([(
                    "decision".to_string(),
                    json!({ "action": "accept", "content": { "decision": "allow" } }),
                )]))
                .with_request_state("forged-token"),
        )
        .await;
    assert!(forged.is_err(), "forged requestState should be rejected");
    Ok(())
}

#[tokio::test]
#[allow(clippy::large_futures)]
async fn coding_permission_retry_with_same_tool_but_changed_arguments_is_rejected_without_side_effect() -> TestResult {
    let root = TempDir::new()?;
    let (client, _event_rx) = elicitation_client("coding-test");
    let mcp = coding_with_permission(mcp_servers::coding::PermissionMode::AlwaysAsk, &root, client).await?;

    // Open an approval round for `touch approved.txt` and capture the opaque
    // requestState the server handed out, without driving any MRTR round.
    let initial = mcp
        .raw()
        .call_tool_once(
            CallToolRequestParams::new("bash")
                .with_arguments(json!({ "command": "touch approved.txt" }).as_object().unwrap().clone()),
        )
        .await?;
    let CallToolResponse::InputRequired(input_required) = initial else {
        panic!("permission-gated bash should open an input round: {initial:?}");
    };
    let token = input_required.request_state.expect("server hands out an opaque requestState");

    // Replay the same token and the same tool name, but swap in a different
    // command. The approval covered the original arguments only, so the server
    // must reject the retry before executing anything.
    let mutated = mcp
        .raw()
        .call_tool(
            CallToolRequestParams::new("bash")
                .with_arguments(json!({ "command": "touch side-effect.txt" }).as_object().unwrap().clone())
                .with_input_responses(rmcp::model::InputResponses::from([(
                    "decision".to_string(),
                    json!({ "action": "accept", "content": { "decision": "allow" } }),
                )]))
                .with_request_state(token),
        )
        .await;
    assert!(mutated.is_err(), "a retry that mutates the approved arguments must be rejected");

    assert!(!root.path().join("approved.txt").exists(), "the originally approved command must not have run");
    assert!(!root.path().join("side-effect.txt").exists(), "the mutated command must not have run");
    Ok(())
}

#[tokio::test]
#[allow(clippy::large_futures)]
async fn coding_permission_retry_with_changed_write_file_arguments_is_rejected_without_side_effect() -> TestResult {
    let root = TempDir::new()?;
    let (client, _event_rx) = elicitation_client("coding-test");
    let mcp = coding_with_permission(mcp_servers::coding::PermissionMode::AlwaysAsk, &root, client).await?;

    // Open an approval round for writing `approved.txt` and capture the
    // opaque requestState, without driving any MRTR round.
    let initial =
        mcp.raw()
            .call_tool_once(CallToolRequestParams::new("write_file").with_arguments(
                json!({ "filePath": "approved.txt", "content": "approved" }).as_object().unwrap().clone(),
            ))
            .await?;
    let CallToolResponse::InputRequired(input_required) = initial else {
        panic!("permission-gated write_file should open an input round: {initial:?}");
    };
    let token = input_required.request_state.expect("server hands out an opaque requestState");

    // Same token, same tool name, but the path and content changed. The
    // approval covered the original arguments only, so the server must reject
    // the retry before any write happens.
    let mutated = mcp
        .raw()
        .call_tool(
            CallToolRequestParams::new("write_file")
                .with_arguments(
                    json!({ "filePath": "side-effect.txt", "content": "evil" }).as_object().unwrap().clone(),
                )
                .with_input_responses(rmcp::model::InputResponses::from([(
                    "decision".to_string(),
                    json!({ "action": "accept", "content": { "decision": "allow" } }),
                )]))
                .with_request_state(token),
        )
        .await;
    assert!(mutated.is_err(), "a retry that mutates the approved write_file arguments must be rejected");

    assert!(!root.path().join("approved.txt").exists(), "the originally approved write must not have run");
    assert!(!root.path().join("side-effect.txt").exists(), "the mutated write must not have run");
    Ok(())
}

#[tokio::test]
#[allow(clippy::large_futures)]
async fn coding_permission_retry_with_changed_edit_file_arguments_is_rejected_without_side_effect() -> TestResult {
    let root = TempDir::new()?;
    let (client, _event_rx) = elicitation_client("coding-test");
    let mcp = coding_with_permission(mcp_servers::coding::PermissionMode::AlwaysAsk, &root, client).await?;

    // Open an approval round for editing `approved.txt` and capture the
    // opaque requestState, without driving any MRTR round.
    let initial = mcp
        .raw()
        .call_tool_once(
            CallToolRequestParams::new("edit_file").with_arguments(
                json!({
                    "filePath": "approved.txt",
                    "edits": [{ "oldString": "one", "newString": "two" }],
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .await?;
    let CallToolResponse::InputRequired(input_required) = initial else {
        panic!("permission-gated edit_file should open an input round: {initial:?}");
    };
    let token = input_required.request_state.expect("server hands out an opaque requestState");

    // Same token, same tool name, but the edit content changed. The approval
    // covered the original arguments only, so the server must reject the retry
    // before any edit happens.
    let mutated = mcp
        .raw()
        .call_tool(
            CallToolRequestParams::new("edit_file")
                .with_arguments(
                    json!({
                        "filePath": "approved.txt",
                        "edits": [{ "oldString": "one", "newString": "evil" }],
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                )
                .with_input_responses(rmcp::model::InputResponses::from([(
                    "decision".to_string(),
                    json!({ "action": "accept", "content": { "decision": "allow" } }),
                )]))
                .with_request_state(token),
        )
        .await;
    assert!(mutated.is_err(), "a retry that mutates the approved edit_file arguments must be rejected");

    assert_eq!(
        std::fs::read_to_string(root.path().join("approved.txt")).unwrap_or_default(),
        "",
        "neither the approved nor the mutated edit may have run"
    );
    Ok(())
}

#[tokio::test]
#[allow(clippy::large_futures)]
async fn coding_permission_exact_retry_runs_the_approved_operation_and_state_binds_arguments() -> TestResult {
    let root = TempDir::new()?;
    let (client, _event_rx) = elicitation_client("coding-test");
    let mcp = coding_with_permission(mcp_servers::coding::PermissionMode::AlwaysAsk, &root, client).await?;

    let initial = mcp
        .raw()
        .call_tool_once(
            CallToolRequestParams::new("bash")
                .with_arguments(json!({ "command": "touch approved.txt" }).as_object().unwrap().clone()),
        )
        .await?;
    let CallToolResponse::InputRequired(input_required) = initial else {
        panic!("permission-gated bash should open an input round: {initial:?}");
    };
    let token = input_required.request_state.expect("server hands out an opaque requestState");

    // An exact replay of the approved operation with `allow` runs the command.
    let approved = mcp
        .raw()
        .call_tool(
            CallToolRequestParams::new("bash")
                .with_arguments(json!({ "command": "touch approved.txt" }).as_object().unwrap().clone())
                .with_input_responses(rmcp::model::InputResponses::from([(
                    "decision".to_string(),
                    json!({ "action": "accept", "content": { "decision": "allow" } }),
                )]))
                .with_request_state(token.clone()),
        )
        .await?;
    assert!(!approved.is_error.unwrap_or(false), "approved bash should complete: {approved:?}");
    assert!(root.path().join("approved.txt").exists(), "the approved command must have run");

    // A valid signed state may be replayed, but it remains bound to the exact operation.
    let replay = mcp
        .raw()
        .call_tool(
            CallToolRequestParams::new("bash")
                .with_arguments(json!({ "command": "touch changed.txt" }).as_object().unwrap().clone())
                .with_input_responses(rmcp::model::InputResponses::from([(
                    "decision".to_string(),
                    json!({ "action": "accept", "content": { "decision": "allow" } }),
                )]))
                .with_request_state(token),
        )
        .await;
    assert!(replay.is_err(), "requestState must not authorize changed arguments");
    Ok(())
}

// Submit-plan review

async fn plan_with_review_response(
    response: ElicitResult,
) -> TestResult<(TempDir, TestClient<PlanMcp, McpClient>, tokio::task::JoinHandle<Option<ElicitRequestParams>>)> {
    let temp_dir = TempDir::new()?;
    let (client, event_rx) = elicitation_client("plan-test");
    let script = respond_to_first_elicitation(event_rx, response);
    let mcp = TestClient::start_with(|| PlanMcp::new().with_plans_dir(temp_dir.path().to_path_buf()), client).await?;
    mcp.call("write_plan", json!({ "planName": "example", "content": "# Plan\n\nShip the feature." })).await?;
    Ok((temp_dir, mcp, script))
}

#[tokio::test]
async fn submit_plan_review_approve_and_deny() -> TestResult {
    let (_temp_dir, mcp, _script) = plan_with_review_response(
        ElicitResult::new(ElicitationAction::Accept).with_content(json!({ "decision": "approve" })),
    )
    .await?;
    let approved = mcp.call("submit_plan", SubmitPlanInput { plan_name: "example".to_string() }).await?;
    assert_eq!(approved["approved"], true, "approve should complete: {approved}");

    let (_temp_dir, mcp, _script) = plan_with_review_response(
        ElicitResult::new(ElicitationAction::Accept).with_content(json!({ "decision": "deny" })),
    )
    .await?;
    let denied = mcp.call("submit_plan", SubmitPlanInput { plan_name: "example".to_string() }).await?;
    assert_eq!(denied["approved"], false, "deny should complete: {denied}");
    Ok(())
}

#[tokio::test]
async fn submit_plan_review_deny_carries_feedback() -> TestResult {
    let (_temp_dir, mcp, _script) =
        plan_with_review_response(ElicitResult::new(ElicitationAction::Accept).with_content(json!({
            "decision": "deny",
            "feedback": "  rewrite the intro  ",
        })))
        .await?;
    let result = mcp.call("submit_plan", SubmitPlanInput { plan_name: "example".to_string() }).await?;
    assert_eq!(result["approved"], false);
    assert_eq!(result["feedback"], "rewrite the intro", "feedback should be trimmed: {result}");
    Ok(())
}

#[tokio::test]
async fn submit_plan_review_decline_and_cancel_complete_unapproved() -> TestResult {
    for action in [ElicitationAction::Decline, ElicitationAction::Cancel] {
        let (_temp_dir, mcp, _script) = plan_with_review_response(ElicitResult::new(action.clone())).await?;
        let result = mcp.call("submit_plan", SubmitPlanInput { plan_name: "example".to_string() }).await?;
        assert_eq!(result["approved"], false, "{action:?} should complete unapproved: {result}");
        assert!(result["feedback"].is_null());
    }
    Ok(())
}

// Survey

async fn survey_with_response(
    response: ElicitResult,
) -> TestResult<(TestClient<SurveyMcp, McpClient>, tokio::task::JoinHandle<Option<ElicitRequestParams>>)> {
    let (client, event_rx) = elicitation_client("survey-test");
    let script = respond_to_first_elicitation(event_rx, response);
    let mcp = TestClient::start_with(SurveyMcp::new, client).await?;
    Ok((mcp, script))
}

fn survey_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "color": { "type": "string", "title": "Favorite color" },
            "rating": { "type": "integer", "title": "Rating" }
        },
        "required": ["color"]
    })
}

#[tokio::test]
async fn survey_accept_returns_data() -> TestResult {
    let (mcp, script) = survey_with_response(
        ElicitResult::new(ElicitationAction::Accept).with_content(json!({ "color": "blue", "rating": 5 })),
    )
    .await?;

    let result = mcp.call("ask_user", json!({ "message": "Pick a color", "schema": survey_schema() })).await?;
    assert_eq!(result["accepted"], true);
    assert_eq!(result["data"]["color"], "blue");

    let request = script.await?.expect("survey should dispatch an elicitation");
    assert!(matches!(request, ElicitRequestParams::FormElicitationParams { .. }));
    Ok(())
}

#[tokio::test]
async fn survey_decline_and_cancel_return_not_accepted() -> TestResult {
    for action in [ElicitationAction::Decline, ElicitationAction::Cancel] {
        let (mcp, _script) = survey_with_response(ElicitResult::new(action.clone())).await?;
        let result = mcp.call("ask_user", json!({ "message": "Pick a color", "schema": survey_schema() })).await?;
        assert_eq!(result["accepted"], false, "{action:?} should not accept: {result}");
        assert!(result["data"].is_null());
    }
    Ok(())
}

#[tokio::test]
async fn survey_missing_required_field_opens_additional_round() -> TestResult {
    let (client, event_rx) = elicitation_client("survey-test");
    let mut event_rx = event_rx;
    let script = tokio::spawn(async move {
        let mut answered = 0usize;
        while let Some(event) = event_rx.recv().await {
            if let McpClientEvent::Elicitation(req) = event {
                answered += 1;
                let response = if answered == 1 {
                    // First answer omits the required `color` field.
                    ElicitResult::new(ElicitationAction::Accept).with_content(json!({ "rating": 3 }))
                } else {
                    ElicitResult::new(ElicitationAction::Accept).with_content(json!({ "color": "green", "rating": 3 }))
                };
                let _ = req.response_sender.send(response);
                if answered >= 2 {
                    return answered;
                }
            }
        }
        answered
    });
    let mcp = TestClient::start_with(SurveyMcp::new, client).await?;

    let result = mcp.call("ask_user", json!({ "message": "Pick a color", "schema": survey_schema() })).await?;
    assert_eq!(result["accepted"], true, "second round should complete: {result}");
    assert_eq!(result["data"]["color"], "green");
    assert_eq!(script.await?, 2, "server should request a second round after invalid input");
    Ok(())
}

#[tokio::test]
async fn survey_retry_with_unknown_state_fails() -> TestResult {
    let (client, _event_rx) = elicitation_client("survey-test");
    let mcp = TestClient::start_with(SurveyMcp::new, client).await?;

    let forged = mcp
        .raw()
        .call_tool(
            rmcp::model::CallToolRequestParams::new("ask_user")
                .with_arguments(json!({ "message": "Pick", "schema": survey_schema() }).as_object().unwrap().clone())
                .with_input_responses(rmcp::model::InputResponses::from([(
                    "form".to_string(),
                    json!({ "action": "accept", "content": { "color": "blue" } }),
                )]))
                .with_request_state("forged-token"),
        )
        .await;
    assert!(forged.is_err(), "forged requestState should be rejected");
    Ok(())
}
