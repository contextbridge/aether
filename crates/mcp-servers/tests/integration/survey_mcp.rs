use crate::common::{TestClient, TestResult, scripted_mcp_client, test_client_info};
use mcp_servers::SurveyMcp;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, ElicitRequestParams, ElicitResult, ElicitationAction, InputResponses,
};
use serde_json::json;

fn ask_user_args(message: &str) -> serde_json::Value {
    json!({
        "message": message,
        "schema": { "type": "object", "properties": { "name": { "type": "string" } }, "required": ["name"] }
    })
}

#[tokio::test]
async fn ask_user_round_trip_returns_accepted_data() -> TestResult {
    let (client, elicitation) = scripted_mcp_client(
        "survey",
        ElicitResult::new(ElicitationAction::Accept).with_content(json!({ "name": "Ada" })),
    );
    let mcp = TestClient::start_with(SurveyMcp::new, client).await?;

    let result = mcp.call("ask_user", ask_user_args("Who are you?")).await?;
    assert_eq!(result["accepted"], true);
    assert_eq!(result["data"]["name"], "Ada");

    let captured = elicitation.captured().pop().expect("an elicitation should be dispatched").request;
    let ElicitRequestParams::FormElicitationParams { message, requested_schema, .. } = captured else {
        panic!("expected form elicitation");
    };
    assert_eq!(message, "Who are you?");
    assert!(requested_schema.properties.contains_key("name"));
    Ok(())
}

#[tokio::test]
async fn ask_user_decline_returns_not_accepted() -> TestResult {
    let (client, _elicitation) = scripted_mcp_client("survey", ElicitResult::new(ElicitationAction::Decline));
    let mcp = TestClient::start_with(SurveyMcp::new, client).await?;

    let result = mcp.call("ask_user", ask_user_args("Who are you?")).await?;
    assert_eq!(result["accepted"], false);
    assert!(result["data"].is_null());
    Ok(())
}

#[tokio::test]
async fn ask_user_manual_two_step_returns_input_required_then_completes() -> TestResult {
    let (client, _elicitation) = scripted_mcp_client("survey", ElicitResult::new(ElicitationAction::Accept));
    let mcp = TestClient::start_with(SurveyMcp::new, client).await?;

    let args = ask_user_args("Who are you?");
    let arguments = args.as_object().unwrap().clone();
    let first =
        mcp.raw().call_tool_once(CallToolRequestParams::new("ask_user").with_arguments(arguments.clone())).await?;

    let CallToolResponse::InputRequired(input_required) = first else {
        panic!("first call should require input, got {first:?}");
    };
    let requests = input_required.input_requests.expect("input requests should be present");
    assert!(requests.contains_key("answer"), "input request should use the 'answer' key: {requests:?}");
    assert!(input_required.request_state.is_none(), "stateless servers should not emit request_state");

    let mut responses = InputResponses::new();
    responses.insert(
        "answer".to_string(),
        serde_json::to_value(ElicitResult::new(ElicitationAction::Accept).with_content(json!({ "name": "Ada" })))?,
    );
    let second = mcp
        .raw()
        .call_tool_once(
            CallToolRequestParams::new("ask_user").with_arguments(arguments).with_input_responses(responses),
        )
        .await?;

    let CallToolResponse::Complete(result) = second else {
        panic!("retry should complete, got {second:?}");
    };
    let structured = result.structured_content.expect("structured output");
    assert_eq!(structured["accepted"], true);
    assert_eq!(structured["data"]["name"], "Ada");
    Ok(())
}

#[tokio::test]
async fn ask_user_without_elicitation_capability_returns_friendly_error() -> TestResult {
    let mcp = TestClient::start_with(SurveyMcp::new, test_client_info()).await?;

    let result = mcp.call_raw("ask_user", ask_user_args("Who are you?")).await?;
    assert!(result.is_error.unwrap_or(false), "expected a tool error: {result:?}");
    let text = result.content.first().and_then(|c| c.as_text()).expect("error text");
    assert!(text.text.contains("does not support"), "error should explain the capability gap: {}", text.text);
    Ok(())
}

#[tokio::test]
async fn ask_user_rejects_invalid_schema_on_first_round() -> TestResult {
    let (client, _elicitation) = scripted_mcp_client("survey", ElicitResult::new(ElicitationAction::Accept));
    let mcp = TestClient::start_with(SurveyMcp::new, client).await?;

    let result = mcp.call_raw("ask_user", json!({ "message": "Hi", "schema": "not json" })).await;
    assert!(result.is_err(), "invalid schema should be a protocol error: {result:?}");
    Ok(())
}
