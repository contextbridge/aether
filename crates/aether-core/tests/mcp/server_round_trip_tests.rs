use aether_core::testing::McpTestBuilder;
use mcp_servers::ReviewMcp;
use rmcp::model::{ElicitResult, ElicitationAction};
use serde_json::json;

#[tokio::test]
async fn artifact_review_round_trips_complete_feedback_through_the_production_executor() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("plan.md");
    std::fs::write(&path, "# Plan\n\nShip it.").unwrap();
    let context = "# Markdown review feedback\n\n## `plan.md`\n\n### Lines 3-4 — Paragraph\n\n```markdown\nShip it.\n```\n\n> Add tests.\n\n### Heading: Plan\n\n> Keep rollout staged.";
    for suffix in ["", "\n", "\n\n"] {
        let feedback = format!("{context}{suffix}");
        let test = McpTestBuilder::new()
            .server("review", ReviewMcp::new().with_root_dir(root.path().to_path_buf()))
            .elicitation_response(
                ElicitResult::new(ElicitationAction::Accept)
                    .with_content(json!({"decision": "feedback", "feedback": feedback})),
            )
            .build()
            .await;
        let result = test
            .call("review", "review_artifact", json!({"path": "plan.md", "format": "markdown"}))
            .await
            .result
            .expect("review completes");
        let output: serde_json::Value = serde_yml::from_str(&result.result).unwrap();
        assert_eq!(output, json!({"status": "feedback", "feedback": feedback}), "encoded result: {:?}", result.result);
        assert_eq!(test.elicitations().len(), 1);
    }
}

#[tokio::test]
async fn artifact_review_round_trips_approval_and_cancellation_through_the_production_executor() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("plan.md");
    std::fs::write(&path, "# Plan").unwrap();
    for (response, status) in [
        (ElicitResult::new(ElicitationAction::Accept).with_content(json!({"decision": "approved"})), "approved"),
        (ElicitResult::new(ElicitationAction::Cancel), "cancelled"),
        (ElicitResult::new(ElicitationAction::Decline), "declined"),
    ] {
        let test = McpTestBuilder::new()
            .server("review", ReviewMcp::new().with_root_dir(root.path().to_path_buf()))
            .elicitation_response(response)
            .build()
            .await;
        let result = test
            .call("review", "review_artifact", json!({"path": "plan.md", "format": "markdown"}))
            .await
            .result
            .expect("review completes");
        let output: serde_json::Value = serde_yml::from_str(&result.result).unwrap();
        assert_eq!(output, json!({"status": status}));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# Plan");
    }
}
