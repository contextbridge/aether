use aether_core::testing::McpTestBuilder;
use mcp_servers::{PlanMcp, SurveyMcp};
use rmcp::model::{ElicitRequestParams, ElicitResult, ElicitationAction};
use serde_json::json;

#[tokio::test]
async fn survey_round_trips_through_the_production_executor() {
    let test = McpTestBuilder::new()
        .server("survey", SurveyMcp::new())
        .elicitation_response(ElicitResult::new(ElicitationAction::Accept).with_content(json!({ "name": "Ada" })))
        .build()
        .await;
    let arguments = json!({
        "message": "Who are you?",
        "schema": {
            "type": "object",
            "properties": { "name": { "type": "string" } },
            "required": ["name"]
        }
    });

    let result = test.call("survey", "ask_user", arguments).await.result.expect("ask_user completes");

    assert!(result.result.contains("Ada"));
    let elicitations = test.elicitations();
    assert_eq!(elicitations.len(), 1);
    assert!(matches!(
        &elicitations[0].request,
        ElicitRequestParams::FormElicitationParams { message, .. } if message == "Who are you?"
    ));
}

#[tokio::test]
async fn plan_review_round_trips_through_the_production_executor() {
    let plans_dir = tempfile::tempdir().unwrap();
    let test = McpTestBuilder::new()
        .server("plan", PlanMcp::new().with_plans_dir(plans_dir.path().to_path_buf()))
        .elicitation_response(
            ElicitResult::new(ElicitationAction::Accept).with_content(json!({ "decision": "approve" })),
        )
        .build()
        .await;

    test.call("plan", "write_plan", json!({ "planName": "vertical", "content": "# Plan\n\nShip it." }))
        .await
        .result
        .expect("write_plan completes");
    let result = test
        .call("plan", "submit_plan", json!({ "planName": "vertical" }))
        .await
        .result
        .expect("submit_plan completes");

    assert!(result.result.contains("approved: true"));
    assert_eq!(test.elicitations().len(), 1);
}
