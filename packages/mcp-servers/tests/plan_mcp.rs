use mcp_servers::{DEFAULT_PLAN_PROMPT, PlanMcp};
use mcp_utils::client::{McpClient, McpClientEvent};
use mcp_utils::testing::connect;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ClientCapabilities, ClientInfo, CreateElicitationRequestParams,
    CreateElicitationResult, ElicitationAction, GetPromptRequestParams, Implementation,
};
use rmcp::service::RunningService;
use rmcp::{RoleClient, Service};
use serde_json::json;
use std::fs;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::{RwLock, mpsc};
use utils::plan_review::PlanReviewElicitationMeta;

#[tokio::test]
async fn submit_plan_attaches_plan_review_metadata_and_preserves_schema() {
    let temp_dir = TempDir::new().expect("create temp dir");
    let plan_content = "# Plan\n\nShip the feature.";

    let server = PlanMcp::new().with_plans_dir(temp_dir.path().to_path_buf());
    let (_server_handle, client_handle) = connect(server, silent_client()).await.expect("connect client");
    write_plan(&client_handle, "example", plan_content).await;

    let (event_tx, event_rx) = mpsc::channel(8);
    let client =
        McpClient::new(test_client_info(), "plan-test-server".to_string(), event_tx, Arc::new(RwLock::new(Vec::new())));

    let task_handle = respond_to_elicitation_request(
        event_rx,
        CreateElicitationResult {
            action: ElicitationAction::Accept,
            content: Some(json!({ "decision": "approve" })),
            meta: None,
        },
    );

    let server = PlanMcp::new().with_plans_dir(temp_dir.path().to_path_buf());
    let (_server_handle, client_handle) = connect(server, client).await.expect("connect client");

    let result = submit_plan(&client_handle, "example").await;
    assert_eq!(result["approved"], true);
    assert!(result["feedback"].is_null());

    let plan_path = temp_dir.path().join("example-plan.md");
    let elicitation_request = task_handle.await.expect("script task panicked").expect("expected elicitation request");
    let CreateElicitationRequestParams::FormElicitationParams { meta, requested_schema, .. } = elicitation_request
    else {
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
}

#[tokio::test]
async fn write_plan_creates_plan_file_from_name() {
    let temp_dir = TempDir::new().expect("create temp dir");
    let server = PlanMcp::new().with_plans_dir(temp_dir.path().to_path_buf());
    let (_server_handle, client_handle) = connect(server, silent_client()).await.expect("connect client");
    let result = write_plan(&client_handle, "auth-refactor", "# Plan\n\nDo it.").await;
    let plan_path = temp_dir.path().join("auth-refactor-plan.md");

    assert_eq!(result["planName"], "auth-refactor");
    assert_eq!(result["planPath"], plan_path.display().to_string());
    assert_eq!(fs::read_to_string(plan_path).expect("read plan"), "# Plan\n\nDo it.");
}

#[tokio::test]
async fn edit_plan_updates_existing_plan_file_from_name() {
    let temp_dir = TempDir::new().expect("create temp dir");
    let server = PlanMcp::new().with_plans_dir(temp_dir.path().to_path_buf());
    let (_server_handle, client_handle) = connect(server, silent_client()).await.expect("connect client");
    write_plan(&client_handle, "auth-refactor", "# Plan\n\nOld approach.").await;

    let result = edit_plan(&client_handle, "auth-refactor", "Old approach", "New approach", false).await;
    let plan_path = temp_dir.path().join("auth-refactor-plan.md");
    assert_eq!(result["replacementsMade"], 1);
    assert_eq!(fs::read_to_string(plan_path).expect("read plan"), "# Plan\n\nNew approach.");
}

#[tokio::test]
async fn plan_name_rejects_path_traversal() {
    let temp_dir = TempDir::new().expect("create temp dir");
    let server = PlanMcp::new().with_plans_dir(temp_dir.path().to_path_buf());
    let (_server_handle, client_handle) = connect(server, silent_client()).await.expect("connect client");
    let request = CallToolRequestParams::new("write_plan")
        .with_arguments(json!({ "planName": "../escape", "content": "# Plan" }).as_object().unwrap().clone());
    let result = client_handle.call_tool(request).await.expect("call write_plan");
    assert!(result.is_error.unwrap_or(false), "expected invalid plan name to be rejected: {result:?}");
}

#[tokio::test]
async fn write_plan_rejects_empty_content() {
    let temp_dir = TempDir::new().expect("create temp dir");
    let server = PlanMcp::new().with_plans_dir(temp_dir.path().to_path_buf());
    let (_server_handle, client_handle) = connect(server, silent_client()).await.expect("connect client");
    let request = CallToolRequestParams::new("write_plan")
        .with_arguments(json!({ "planName": "blank", "content": "   \n\t" }).as_object().unwrap().clone());
    let result = client_handle.call_tool(request).await.expect("call write_plan");

    assert!(result.is_error.unwrap_or(false), "expected empty content to be rejected: {result:?}");
}

#[tokio::test]
async fn submit_plan_errors_on_missing_plan() {
    let temp_dir = TempDir::new().expect("create temp dir");
    let server = PlanMcp::new().with_plans_dir(temp_dir.path().to_path_buf());
    let (_server_handle, client_handle) = connect(server, silent_client()).await.expect("connect client");
    let result = submit_plan_raw(&client_handle, "missing").await;
    assert!(result.is_error.unwrap_or(false), "expected error for missing plan: {result:?}");
}

fn test_client_info() -> ClientInfo {
    ClientInfo::new(ClientCapabilities::default(), Implementation::new("plan-test-client", "0.1.0"))
}

fn respond_to_elicitation_request(
    mut event_rx: mpsc::Receiver<McpClientEvent>,
    response: CreateElicitationResult,
) -> tokio::task::JoinHandle<Option<CreateElicitationRequestParams>> {
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

async fn submit_plan<T: Service<RoleClient>>(
    client: &RunningService<RoleClient, T>,
    plan_name: &str,
) -> serde_json::Value {
    let request = CallToolRequestParams::new("submit_plan")
        .with_arguments(json!({ "planName": plan_name }).as_object().unwrap().clone());

    let result = client.call_tool(request).await.expect("call submit_plan");
    let content = result.content.first().expect("Expected content");
    let text = content.as_text().expect("Expected text content");
    serde_json::from_str(&text.text).expect("Invalid JSON response")
}

async fn submit_plan_raw<T: Service<RoleClient>>(
    client: &RunningService<RoleClient, T>,
    plan_name: &str,
) -> CallToolResult {
    let request = CallToolRequestParams::new("submit_plan")
        .with_arguments(json!({ "planName": plan_name }).as_object().unwrap().clone());
    client.call_tool(request).await.expect("call submit_plan")
}

async fn write_plan<T: Service<RoleClient>>(
    client: &RunningService<RoleClient, T>,
    plan_name: &str,
    content: &str,
) -> serde_json::Value {
    let request = CallToolRequestParams::new("write_plan")
        .with_arguments(json!({ "planName": plan_name, "content": content }).as_object().unwrap().clone());

    let result = client.call_tool(request).await.expect("call write_plan");
    let content = result.content.first().expect("Expected content");
    let text = content.as_text().expect("Expected text content");
    serde_json::from_str(&text.text).expect("Invalid JSON response")
}

async fn edit_plan<T: Service<RoleClient>>(
    client: &RunningService<RoleClient, T>,
    plan_name: &str,
    old_string: &str,
    new_string: &str,
    replace_all: bool,
) -> serde_json::Value {
    let request = CallToolRequestParams::new("edit_plan").with_arguments(
        json!({
            "planName": plan_name,
            "oldString": old_string,
            "newString": new_string,
            "replaceAll": replace_all
        })
        .as_object()
        .unwrap()
        .clone(),
    );

    let result = client.call_tool(request).await.expect("call edit_plan");
    let content = result.content.first().expect("Expected content");
    let text = content.as_text().expect("Expected text content");
    serde_json::from_str(&text.text).expect("Invalid JSON response")
}

fn silent_client() -> McpClient {
    let (event_tx, _event_rx) = mpsc::channel(8);
    McpClient::new(test_client_info(), "plan-test-server".to_string(), event_tx, Arc::new(RwLock::new(Vec::new())))
}

#[tokio::test]
async fn list_prompts_returns_plan_prompt() {
    let (_server_handle, client_handle) = connect(PlanMcp::new(), silent_client()).await.expect("connect client");
    let result = client_handle.list_prompts(None).await.expect("list prompts");

    assert_eq!(result.prompts.len(), 1);
    assert_eq!(result.prompts[0].name, "plan");

    let args = result.prompts[0].arguments.as_ref().expect("plan prompt should advertise arguments");
    assert_eq!(args.len(), 1);
    assert_eq!(args[0].name, "ARGUMENTS");
}

#[tokio::test]
async fn get_prompt_returns_default_body_when_unconfigured() {
    let (_server_handle, client_handle) = connect(PlanMcp::new(), silent_client()).await.expect("connect client");
    let result = client_handle.get_prompt(GetPromptRequestParams::new("plan")).await.expect("get prompt");

    assert_eq!(result.messages.len(), 1);
    let text = extract_user_text(&result.messages[0]);
    assert_eq!(text, DEFAULT_PLAN_PROMPT);
}

#[tokio::test]
async fn get_prompt_substitutes_arguments() {
    let (_server_handle, client_handle) = connect(PlanMcp::new(), silent_client()).await.expect("connect client");

    let args = json!({ "ARGUMENTS": "wire up the widget" }).as_object().unwrap().clone();
    let request = GetPromptRequestParams::new("plan").with_arguments(args);
    let result = client_handle.get_prompt(request).await.expect("get prompt");

    let text = extract_user_text(&result.messages[0]);
    assert!(text.contains("<task>wire up the widget</task>"), "expected substituted task in: {text}");
    assert!(!text.contains("$ARGUMENTS"), "expected $ARGUMENTS placeholder to be gone in: {text}");
}

#[tokio::test]
async fn get_prompt_uses_configured_prompt_file() {
    let temp_dir = TempDir::new().expect("tempdir");
    let path = temp_dir.path().join("custom.md");
    fs::write(&path, "custom plan mode body").expect("write custom prompt");

    let server = PlanMcp::new().with_prompt_file(path);
    let (_server_handle, client_handle) = connect(server, silent_client()).await.expect("connect client");

    let result = client_handle.get_prompt(GetPromptRequestParams::new("plan")).await.expect("get prompt");
    assert_eq!(extract_user_text(&result.messages[0]), "custom plan mode body");
}

#[tokio::test]
async fn get_prompt_falls_back_when_configured_file_missing() {
    let temp_dir = TempDir::new().expect("tempdir");
    let missing = temp_dir.path().join("not-there.md");

    let server = PlanMcp::new().with_prompt_file(missing);
    let (_server_handle, client_handle) = connect(server, silent_client()).await.expect("connect client");

    let result = client_handle.get_prompt(GetPromptRequestParams::new("plan")).await.expect("get prompt");
    assert_eq!(extract_user_text(&result.messages[0]), DEFAULT_PLAN_PROMPT);
}

fn extract_user_text(message: &rmcp::model::PromptMessage) -> String {
    match &message.content {
        rmcp::model::PromptMessageContent::Text { text } => text.clone(),
        other => panic!("expected text content, got {other:?}"),
    }
}

#[tokio::test]
async fn submit_plan_runs_external_command_and_forwards_stdout_as_feedback() {
    let temp_dir = TempDir::new().expect("tempdir");
    let plan_path = temp_dir.path().join("plan-plan.md");
    fs::write(&plan_path, "# Plan\n\nDo the thing.").expect("write plan");

    let server = PlanMcp::new().with_plans_dir(temp_dir.path().to_path_buf()).with_submit_command(vec![
        "/bin/sh".into(),
        "-c".into(),
        r#"printf "feedback for %s" "$1""#.into(),
        "--".into(),
    ]);
    let (_server, client) = connect(server, silent_client()).await.expect("connect");

    let result = submit_plan(&client, "plan").await;
    assert_eq!(result["approved"], false);
    let feedback = result["feedback"].as_str().expect("feedback string");
    let expected = format!("feedback for {}", plan_path.display());
    assert_eq!(feedback, expected, "expected verbatim stdout forwarded to feedback");
}

#[tokio::test]
async fn submit_plan_external_command_forwards_empty_stdout() {
    let temp_dir = TempDir::new().expect("tempdir");
    let plan_path = temp_dir.path().join("plan-plan.md");
    fs::write(&plan_path, "# Plan").expect("write plan");

    let server =
        PlanMcp::new().with_plans_dir(temp_dir.path().to_path_buf()).with_submit_command(vec!["/usr/bin/true".into()]);
    let (_server, client) = connect(server, silent_client()).await.expect("connect");

    let result = submit_plan(&client, "plan").await;
    assert_eq!(result["approved"], false);
    assert_eq!(result["feedback"], "", "empty stdout should still be forwarded as empty feedback");
}

#[tokio::test]
async fn submit_plan_external_command_errors_on_nonzero_exit() {
    let temp_dir = TempDir::new().expect("tempdir");
    let plan_path = temp_dir.path().join("plan-plan.md");
    fs::write(&plan_path, "# Plan").expect("write plan");

    let server =
        PlanMcp::new().with_plans_dir(temp_dir.path().to_path_buf()).with_submit_command(vec!["/usr/bin/false".into()]);
    let (_server, client) = connect(server, silent_client()).await.expect("connect");

    let result = submit_plan_raw(&client, "plan").await;
    assert!(result.is_error.unwrap_or(false), "expected tool error for nonzero exit: {result:?}");
}

#[tokio::test]
async fn submit_plan_external_command_errors_on_missing_binary() {
    let temp_dir = TempDir::new().expect("tempdir");
    let plan_path = temp_dir.path().join("plan-plan.md");
    fs::write(&plan_path, "# Plan").expect("write plan");

    let server = PlanMcp::new()
        .with_plans_dir(temp_dir.path().to_path_buf())
        .with_submit_command(vec!["aether-plan-mcp-does-not-exist-xyz".into()]);
    let (_server, client) = connect(server, silent_client()).await.expect("connect");

    let result = submit_plan_raw(&client, "plan").await;
    assert!(result.is_error.unwrap_or(false), "expected tool error for missing binary: {result:?}");
}
