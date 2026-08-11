use crate::common::{TestClient, TestResult, scripted_mcp_client, silent_mcp_client, test_client_info};
use mcp_servers::file_ops::FileEdit;
use mcp_servers::plan::{EditPlanInput, SubmitPlanInput, WritePlanInput};
use mcp_servers::{DEFAULT_PLAN_PROMPT, PlanMcp};
use mcp_utils::client::McpClient;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, ElicitRequestParams, ElicitResult, ElicitationAction,
    GetPromptRequestParams, InputRequest, InputResponses,
};
use serde_json::json;
use std::fs;
use tempfile::TempDir;
use utils::plan_review::PlanReviewElicitationMeta;

fn silent_client() -> McpClient {
    silent_mcp_client("plan-test-server")
}

fn write_plan_input(plan_name: &str, content: &str) -> WritePlanInput {
    WritePlanInput { plan_name: plan_name.to_string(), content: content.to_string() }
}

fn edit_plan_input(plan_name: &str, edits: Vec<FileEdit>) -> EditPlanInput {
    EditPlanInput { plan_name: plan_name.to_string(), edits }
}

fn submit_plan_input(plan_name: &str) -> SubmitPlanInput {
    SubmitPlanInput { plan_name: plan_name.to_string() }
}

async fn submit_plan_raw(mcp: &TestClient<PlanMcp, McpClient>, plan_name: &str) -> rmcp::model::CallToolResult {
    mcp.call_raw("submit_plan", submit_plan_input(plan_name)).await.expect("call submit_plan")
}

#[tokio::test]
async fn submit_plan_attaches_plan_review_metadata_and_preserves_schema() -> TestResult {
    let temp_dir = TempDir::new()?;
    let plan_content = "# Plan\n\nShip the feature.";

    let mcp = TestClient::start_with(|| PlanMcp::new().with_plans_dir(temp_dir.path().to_path_buf()), silent_client())
        .await?;
    mcp.call("write_plan", write_plan_input("example", plan_content)).await?;

    let (client, elicitation) = scripted_mcp_client(
        "plan-test-server",
        ElicitResult::new(ElicitationAction::Accept).with_content(json!({ "decision": "approve" })),
    );

    let mcp = TestClient::start_with(|| PlanMcp::new().with_plans_dir(temp_dir.path().to_path_buf()), client).await?;
    let result = mcp.call("submit_plan", submit_plan_input("example")).await?;
    assert_eq!(result["approved"], true);
    assert!(result["feedback"].is_null());

    let plan_path = temp_dir.path().join("example-plan.md");
    let elicitation_request = elicitation.captured().pop().expect("expected elicitation request").request;
    let ElicitRequestParams::FormElicitationParams { meta, requested_schema, .. } = elicitation_request else {
        panic!("submit_plan should issue form elicitation request");
    };

    let required = requested_schema.required.clone().unwrap_or_default();
    assert_eq!(required, vec!["decision".to_string()]);
    assert!(requested_schema.properties.contains_key("decision"));
    assert!(requested_schema.properties.contains_key("feedback"));

    let meta = meta.expect("plan review metadata should be set");
    let parsed_meta = PlanReviewElicitationMeta::parse(Some(&meta.0)).expect("should parse plan review metadata");
    assert_eq!(parsed_meta.ui, "planReview");
    assert_eq!(parsed_meta.plan_path, plan_path.display().to_string());
    assert_eq!(parsed_meta.markdown, plan_content);
    Ok(())
}

#[tokio::test]
async fn submit_plan_manual_two_step_keeps_review_meta_and_completes() -> TestResult {
    let temp_dir = TempDir::new()?;
    let mcp = TestClient::start_with(|| PlanMcp::new().with_plans_dir(temp_dir.path().to_path_buf()), silent_client())
        .await?;
    mcp.call("write_plan", write_plan_input("example", "# Plan\n\nShip it.")).await?;

    let first = mcp
        .raw()
        .call_tool_once(
            CallToolRequestParams::new("submit_plan")
                .with_arguments(json!({ "planName": "example" }).as_object().unwrap().clone()),
        )
        .await?;

    let CallToolResponse::InputRequired(input_required) = first else {
        panic!("first submit_plan call should require input, got {first:?}");
    };
    assert!(input_required.request_state.is_none(), "stateless servers should not emit request_state");
    let requests = input_required.input_requests.expect("input requests should be present");
    let InputRequest::Elicitation(elicit) = requests.get("review").expect("input request keyed 'review'") else {
        panic!("expected an elicitation input request");
    };
    let ElicitRequestParams::FormElicitationParams { meta, .. } = &elicit.params else {
        panic!("expected form elicitation");
    };
    let meta = meta.as_ref().expect("plan review metadata should survive MRTR transport");
    PlanReviewElicitationMeta::parse(Some(&meta.0)).expect("should parse plan review metadata");

    let mut responses = InputResponses::new();
    responses.insert(
        "review".to_string(),
        serde_json::to_value(
            ElicitResult::new(ElicitationAction::Accept)
                .with_content(json!({ "decision": "deny", "feedback": "needs tests" })),
        )?,
    );
    let second = mcp
        .raw()
        .call_tool_once(
            CallToolRequestParams::new("submit_plan")
                .with_arguments(json!({ "planName": "example" }).as_object().unwrap().clone())
                .with_input_responses(responses),
        )
        .await?;

    let CallToolResponse::Complete(result) = second else {
        panic!("retry should complete, got {second:?}");
    };
    let structured = result.structured_content.expect("structured output");
    assert_eq!(structured["approved"], false);
    assert_eq!(structured["feedback"], "needs tests");
    Ok(())
}

#[tokio::test]
async fn submit_plan_for_missing_plan_returns_tool_error() -> TestResult {
    let temp_dir = TempDir::new()?;
    let mcp = TestClient::start_with(|| PlanMcp::new().with_plans_dir(temp_dir.path().to_path_buf()), silent_client())
        .await?;

    let result = mcp.call_raw("submit_plan", submit_plan_input("missing")).await?;
    assert!(result.is_error.unwrap_or(false), "missing plan should be a tool error: {result:?}");
    let text = result.content.first().and_then(|c| c.as_text()).expect("error text");
    assert!(text.text.contains("missing"), "error should name the plan: {}", text.text);
    Ok(())
}

#[tokio::test]
async fn submit_plan_without_elicitation_capability_returns_friendly_error() -> TestResult {
    let temp_dir = TempDir::new()?;
    let mcp =
        TestClient::start_with(|| PlanMcp::new().with_plans_dir(temp_dir.path().to_path_buf()), test_client_info())
            .await?;
    mcp.call("write_plan", write_plan_input("example", "# Plan")).await?;

    let result = mcp.call_raw("submit_plan", submit_plan_input("example")).await?;
    assert!(result.is_error.unwrap_or(false), "expected a tool error: {result:?}");
    let text = result.content.first().and_then(|c| c.as_text()).expect("error text");
    assert!(text.text.contains("does not support"), "error should explain the capability gap: {}", text.text);
    Ok(())
}

#[tokio::test]
async fn write_plan_creates_plan_file_from_name() -> TestResult {
    let temp_dir = TempDir::new()?;
    let mcp = TestClient::start_with(|| PlanMcp::new().with_plans_dir(temp_dir.path().to_path_buf()), silent_client())
        .await?;
    let result = mcp.call("write_plan", write_plan_input("auth-refactor", "# Plan\n\nDo it.")).await?;
    let plan_path = temp_dir.path().join("auth-refactor-plan.md");

    assert_eq!(result["planName"], "auth-refactor");
    assert_eq!(result["planPath"], plan_path.display().to_string());
    assert_eq!(fs::read_to_string(plan_path)?, "# Plan\n\nDo it.");
    Ok(())
}

#[tokio::test]
async fn edit_plan_updates_existing_plan_file_from_name() -> TestResult {
    let temp_dir = TempDir::new()?;
    let mcp = TestClient::start_with(|| PlanMcp::new().with_plans_dir(temp_dir.path().to_path_buf()), silent_client())
        .await?;
    mcp.call("write_plan", write_plan_input("auth-refactor", "# Plan\n\nOld approach.")).await?;

    let result = mcp
        .call(
            "edit_plan",
            edit_plan_input(
                "auth-refactor",
                vec![FileEdit {
                    old_string: "Old approach".to_string(),
                    new_string: "New approach".to_string(),
                    replace_all: false,
                }],
            ),
        )
        .await?;
    let plan_path = temp_dir.path().join("auth-refactor-plan.md");
    assert_eq!(result["replacementsMade"], 1);
    assert_eq!(fs::read_to_string(plan_path)?, "# Plan\n\nNew approach.");
    Ok(())
}

#[tokio::test]
async fn edit_plan_applies_batch_in_single_call() -> TestResult {
    let temp_dir = TempDir::new()?;
    let mcp = TestClient::start_with(|| PlanMcp::new().with_plans_dir(temp_dir.path().to_path_buf()), silent_client())
        .await?;
    mcp.call("write_plan", write_plan_input("feature", "# Plan\nStep one\nStep two\n")).await?;

    let parsed = mcp
        .call(
            "edit_plan",
            edit_plan_input(
                "feature",
                vec![FileEdit::new("Step one", "Step ONE"), FileEdit::new("Step two", "Step TWO")],
            ),
        )
        .await?;

    assert_eq!(parsed["replacementsMade"], 2);
    let plan_path = temp_dir.path().join("feature-plan.md");
    assert_eq!(fs::read_to_string(plan_path)?, "# Plan\nStep ONE\nStep TWO\n");
    Ok(())
}

#[tokio::test]
async fn plan_name_rejects_path_traversal() -> TestResult {
    let temp_dir = TempDir::new()?;
    let mcp = TestClient::start_with(|| PlanMcp::new().with_plans_dir(temp_dir.path().to_path_buf()), silent_client())
        .await?;
    let result = mcp.call_raw("write_plan", write_plan_input("../escape", "# Plan")).await?;
    assert!(result.is_error.unwrap_or(false), "expected invalid plan name to be rejected: {result:?}");
    Ok(())
}

#[tokio::test]
async fn write_plan_rejects_empty_content() -> TestResult {
    let temp_dir = TempDir::new()?;
    let mcp = TestClient::start_with(|| PlanMcp::new().with_plans_dir(temp_dir.path().to_path_buf()), silent_client())
        .await?;
    let result = mcp.call_raw("write_plan", write_plan_input("blank", "   \n\t")).await?;

    assert!(result.is_error.unwrap_or(false), "expected empty content to be rejected: {result:?}");
    Ok(())
}

#[tokio::test]
async fn submit_plan_errors_on_missing_plan() {
    let temp_dir = TempDir::new().expect("create temp dir");
    let mcp = TestClient::start_with(|| PlanMcp::new().with_plans_dir(temp_dir.path().to_path_buf()), silent_client())
        .await
        .expect("connect client");
    let result = submit_plan_raw(&mcp, "missing").await;
    assert!(result.is_error.unwrap_or(false), "expected error for missing plan: {result:?}");
}

#[tokio::test]
async fn list_prompts_returns_plan_prompt() {
    let mcp = TestClient::start_with(PlanMcp::new, silent_client()).await.expect("connect client");
    let result = mcp.raw().list_prompts(None).await.expect("list prompts");

    assert_eq!(result.prompts.len(), 1);
    assert_eq!(result.prompts[0].name, "plan");

    let args = result.prompts[0].arguments.as_ref().expect("plan prompt should advertise arguments");
    assert_eq!(args.len(), 1);
    assert_eq!(args[0].name, "ARGUMENTS");
}

#[tokio::test]
async fn get_prompt_returns_default_body_when_unconfigured() {
    let mcp = TestClient::start_with(PlanMcp::new, silent_client()).await.expect("connect client");
    let result = mcp.raw().get_prompt(GetPromptRequestParams::new("plan")).await.expect("get prompt");

    assert_eq!(result.messages.len(), 1);
    let text = extract_user_text(&result.messages[0]);
    assert_eq!(text, DEFAULT_PLAN_PROMPT);
}

#[tokio::test]
async fn get_prompt_substitutes_arguments() {
    let mcp = TestClient::start_with(PlanMcp::new, silent_client()).await.expect("connect client");

    let args = json!({ "ARGUMENTS": "wire up the widget" }).as_object().unwrap().clone();
    let request = GetPromptRequestParams::new("plan").with_arguments(args);
    let result = mcp.raw().get_prompt(request).await.expect("get prompt");

    let text = extract_user_text(&result.messages[0]);
    assert!(text.contains("<task>wire up the widget</task>"), "expected substituted task in: {text}");
    assert!(!text.contains("$ARGUMENTS"), "expected $ARGUMENTS placeholder to be gone in: {text}");
}

#[tokio::test]
async fn get_prompt_uses_configured_prompt_file() {
    let temp_dir = TempDir::new().expect("tempdir");
    let path = temp_dir.path().join("custom.md");
    fs::write(&path, "custom plan mode body").expect("write custom prompt");

    let mcp = TestClient::start_with(|| PlanMcp::new().with_prompt_file(path), silent_client())
        .await
        .expect("connect client");
    let result = mcp.raw().get_prompt(GetPromptRequestParams::new("plan")).await.expect("get prompt");
    assert_eq!(extract_user_text(&result.messages[0]), "custom plan mode body");
}

#[tokio::test]
async fn get_prompt_falls_back_when_configured_file_missing() {
    let temp_dir = TempDir::new().expect("tempdir");
    let missing = temp_dir.path().join("not-there.md");

    let mcp = TestClient::start_with(|| PlanMcp::new().with_prompt_file(missing), silent_client())
        .await
        .expect("connect client");
    let result = mcp.raw().get_prompt(GetPromptRequestParams::new("plan")).await.expect("get prompt");
    assert_eq!(extract_user_text(&result.messages[0]), DEFAULT_PLAN_PROMPT);
}

fn extract_user_text(message: &rmcp::model::PromptMessage) -> String {
    match &message.content {
        rmcp::model::ContentBlock::Text(text) => text.text.clone(),
        other => panic!("expected text content, got {other:?}"),
    }
}

#[tokio::test]
async fn submit_plan_runs_external_command_and_forwards_stdout_as_feedback() -> TestResult {
    let temp_dir = TempDir::new()?;
    let plan_path = temp_dir.path().join("plan-plan.md");
    fs::write(&plan_path, "# Plan\n\nDo the thing.")?;

    let mcp = TestClient::start_with(
        || {
            PlanMcp::new().with_plans_dir(temp_dir.path().to_path_buf()).with_submit_command(vec![
                "/bin/sh".into(),
                "-c".into(),
                r#"printf "feedback for %s" "$1""#.into(),
                "--".into(),
            ])
        },
        silent_client(),
    )
    .await?;

    let result = mcp.call("submit_plan", submit_plan_input("plan")).await?;
    assert_eq!(result["approved"], false);
    let feedback = result["feedback"].as_str().expect("feedback string");
    let expected = format!("feedback for {}", plan_path.display());
    assert_eq!(feedback, expected, "expected verbatim stdout forwarded to feedback");
    Ok(())
}

#[tokio::test]
async fn submit_plan_external_command_forwards_empty_stdout() -> TestResult {
    let temp_dir = TempDir::new()?;
    let plan_path = temp_dir.path().join("plan-plan.md");
    fs::write(&plan_path, "# Plan")?;

    let mcp = TestClient::start_with(
        || {
            PlanMcp::new()
                .with_plans_dir(temp_dir.path().to_path_buf())
                .with_submit_command(vec!["/usr/bin/true".into()])
        },
        silent_client(),
    )
    .await?;

    let result = mcp.call("submit_plan", submit_plan_input("plan")).await?;
    assert_eq!(result["approved"], false);
    assert_eq!(result["feedback"], "", "empty stdout should still be forwarded as empty feedback");
    Ok(())
}

#[tokio::test]
async fn submit_plan_external_command_errors_on_nonzero_exit() -> TestResult {
    let temp_dir = TempDir::new()?;
    let plan_path = temp_dir.path().join("plan-plan.md");
    fs::write(&plan_path, "# Plan")?;

    let mcp = TestClient::start_with(
        || {
            PlanMcp::new()
                .with_plans_dir(temp_dir.path().to_path_buf())
                .with_submit_command(vec!["/usr/bin/false".into()])
        },
        silent_client(),
    )
    .await?;

    let result = submit_plan_raw(&mcp, "plan").await;
    assert!(result.is_error.unwrap_or(false), "expected tool error for nonzero exit: {result:?}");
    Ok(())
}

#[tokio::test]
async fn submit_plan_external_command_errors_on_missing_binary() -> TestResult {
    let temp_dir = TempDir::new()?;
    let plan_path = temp_dir.path().join("plan-plan.md");
    fs::write(&plan_path, "# Plan")?;

    let mcp = TestClient::start_with(
        || {
            PlanMcp::new()
                .with_plans_dir(temp_dir.path().to_path_buf())
                .with_submit_command(vec!["aether-plan-mcp-does-not-exist-xyz".into()])
        },
        silent_client(),
    )
    .await?;

    let result = submit_plan_raw(&mcp, "plan").await;
    assert!(result.is_error.unwrap_or(false), "expected tool error for missing binary: {result:?}");
    Ok(())
}
