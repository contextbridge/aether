use super::support::call_tool;
use aether_core::testing::test_agent;
use mcp_utils::testing::{FakeMcpServer, FakeTool, FakeToolResponse, accept_with_name, name_input};
use serde_json::json;

#[tokio::test]
async fn input_requests_are_elicited_from_the_host_and_answered() {
    let server = FakeMcpServer::new().with_tool(
        FakeTool::new("form").responds(name_input("asked")).when_state("asked", FakeToolResponse::text("done")),
    );
    let state = server.state();
    let agent = test_agent().mcp_server("mrtr", server).elicitation_response(accept_with_name("Ferris"));

    let run = call_tool(agent, "mrtr__form", json!({})).await;

    assert!(run.result().expect("the input round trip completes").result.contains("done"));
    let answers = state.calls_for("form")[1].request.input_responses.clone().expect("the tool was answered");
    assert_eq!(answers["name"]["content"]["name"], "Ferris");
    assert_eq!(run.elicitations.len(), 1);
    assert_eq!(run.elicitations[0].server_name, "mrtr");
}
