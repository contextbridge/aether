use aether_core::testing::{FakeMcpServer, FakeTool, FakeToolResponse, McpTestBuilder};
use rmcp::model::{
    CallToolResponse, ElicitRequest, ElicitRequestParams, ElicitResult, ElicitationAction, InputRequest, InputRequests,
    InputRequiredResult,
};
use serde_json::json;

const TRICKY_STATE: &str = "st.ate/with+special=chars and spaces \"quotes\"\n\ttab";

#[tokio::test]
async fn form_round_trip_forwards_response_and_echoes_state() {
    let server = FakeMcpServer::new().with_tool(
        FakeTool::new("form")
            .responds(input_required("answer", form_request("Name?"), TRICKY_STATE))
            .when_state(TRICKY_STATE, FakeToolResponse::text("done")),
    );
    let state = server.state();
    let test =
        McpTestBuilder::new().server("mrtr", server).elicitation_response(accept_with_name("Ferris")).build().await;

    let result = test.call("mrtr", "form", json!({})).await.result.expect("form round trip completes");

    assert!(result.result.contains("done"));
    let calls = state.calls_for("form");
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1].request.request_state.as_deref(), Some(TRICKY_STATE));
    assert_eq!(calls[1].request.input_responses.as_ref().unwrap()["answer"]["action"], "accept");
    assert_eq!(calls[1].request.input_responses.as_ref().unwrap()["answer"]["content"]["name"], "Ferris");
    let elicitations = test.elicitations();
    assert_eq!(elicitations.len(), 1);
    assert_eq!(elicitations[0].server_name, "mrtr");
    assert!(matches!(
        &elicitations[0].request,
        ElicitRequestParams::FormElicitationParams { message, .. } if message == "Name?"
    ));
}

#[tokio::test]
async fn url_elicitation_and_state_only_polling_complete_without_reprompting() {
    let server = FakeMcpServer::new().with_tool(
        FakeTool::new("url_then_poll")
            .responds(input_required(
                "consent",
                url_request("Authorize to continue", "https://example.com/auth", "el-1"),
                "url-opened",
            ))
            .when_state("url-opened", InputRequiredResult::from_request_state("poll-1"))
            .when_state("poll-1", FakeToolResponse::text("url-done")),
    );
    let state = server.state();
    let test = McpTestBuilder::new()
        .server("mrtr", server)
        .elicitation_response(ElicitResult::new(ElicitationAction::Accept))
        .build()
        .await;

    let result = test.call("mrtr", "url_then_poll", json!({})).await.result.expect("URL flow completes");

    assert!(result.result.contains("url-done"));
    assert_eq!(state.calls_for("url_then_poll").len(), 3);
    let elicitations = test.elicitations();
    assert_eq!(elicitations.len(), 1, "state-only polling must not prompt again");
    assert!(matches!(
        &elicitations[0].request,
        ElicitRequestParams::UrlElicitationParams { url, elicitation_id, .. }
            if url == "https://example.com/auth" && elicitation_id == "el-1"
    ));
}

#[tokio::test]
async fn progress_notifications_are_forwarded_across_rounds() {
    let server = FakeMcpServer::new().with_tool(
        FakeTool::new("progress_rounds")
            .responds(
                FakeToolResponse::new(input_required("answer", form_request("Name?"), "progress"))
                    .progress(1.0, Some(2.0)),
            )
            .when_state("progress", FakeToolResponse::text("progress-done").progress(2.0, Some(2.0))),
    );
    let test =
        McpTestBuilder::new().server("mrtr", server).elicitation_response(accept_with_name("Ferris")).build().await;

    let outcome = test.call("mrtr", "progress_rounds", json!({})).await;

    assert!(outcome.result.expect("progress tool completes").result.contains("progress-done"));
    assert_eq!(outcome.progress.iter().map(|event| event.progress).collect::<Vec<_>>(), vec![1.0, 2.0]);
}

#[tokio::test]
async fn invalid_input_required_result_becomes_a_tool_error() {
    let server =
        FakeMcpServer::new().with_tool(FakeTool::new("invalid").responds(InputRequiredResult::new(None, None)));
    let test = McpTestBuilder::new().server("mrtr", server).build().await;

    let error = test.call("mrtr", "invalid", json!({})).await.result.expect_err("invalid result fails");

    assert!(error.error.contains("without any input requests or request state"), "{}", error.error);
}

fn form_request(message: &str) -> InputRequest {
    InputRequest::Elicitation(ElicitRequest::new(ElicitRequestParams::FormElicitationParams {
        meta: None,
        message: message.to_string(),
        requested_schema: serde_json::from_value(json!({
            "type": "object",
            "properties": { "name": { "type": "string" } },
            "required": ["name"]
        }))
        .unwrap(),
    }))
}

fn url_request(message: &str, url: &str, elicitation_id: &str) -> InputRequest {
    InputRequest::Elicitation(ElicitRequest::new(ElicitRequestParams::UrlElicitationParams {
        meta: None,
        message: message.to_string(),
        url: url.to_string(),
        elicitation_id: elicitation_id.to_string(),
    }))
}

fn input_required(key: &str, request: InputRequest, state: &str) -> CallToolResponse {
    let mut requests = InputRequests::new();
    requests.insert(key.to_string(), request);
    InputRequiredResult::new(Some(requests), Some(state.to_string())).into()
}

fn accept_with_name(name: &str) -> ElicitResult {
    ElicitResult::new(ElicitationAction::Accept).with_content(json!({ "name": name }))
}
