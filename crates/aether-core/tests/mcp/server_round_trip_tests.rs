use super::support::call_tool;
use aether_core::testing::test_agent;
use mcp_servers::ReviewMcp;
use rmcp::model::{ElicitResult, ElicitationAction};
use serde_json::json;
use std::path::Path;

#[tokio::test]
async fn artifact_review_round_trips_complete_feedback_through_the_production_executor() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("plan.md");
    std::fs::write(&path, "# Plan\n\nShip it.").unwrap();
    let context = "# Markdown review feedback\n\n## `plan.md`\n\n### Lines 3-4 — Paragraph\n\n```markdown\nShip it.\n```\n\n> Add tests.\n\n### Heading: Plan\n\n> Keep rollout staged.";
    for suffix in ["", "\n", "\n\n"] {
        let feedback = format!("{context}{suffix}");
        let agent = test_agent().mcp_server("review", review_mcp_at(root.path())).elicitation_response(
            ElicitResult::new(ElicitationAction::Accept)
                .with_content(json!({"decision": "feedback", "feedback": feedback})),
        );

        let run = call_tool(agent, REVIEW_ARTIFACT, markdown_plan()).await;

        let result = run.result().expect("review completes");
        let output: serde_json::Value = noyalib::from_str(&result.result).unwrap();
        assert_eq!(output, json!({"status": "feedback", "feedback": feedback}), "encoded result: {:?}", result.result);
        assert_eq!(run.elicitations.len(), 1);
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
        let agent = test_agent().mcp_server("review", review_mcp_at(root.path())).elicitation_response(response);

        let run = call_tool(agent, REVIEW_ARTIFACT, markdown_plan()).await;

        let output: serde_json::Value = noyalib::from_str(&run.result().expect("review completes").result).unwrap();
        assert_eq!(output, json!({"status": status}));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# Plan");
    }
}

#[tokio::test]
async fn artifact_review_round_trips_html_annotations_through_the_production_executor() {
    let root = tempfile::tempdir().unwrap();
    let submission = json!({
        "status": "feedback",
        "feedback": "Tighten the hero.",
        "annotations": [{"element": "<h1>", "excerpt": "Ship faster", "comment": "Make this larger."}]
    });
    let browser = submission.clone();
    let agent = test_agent()
        .mcp_server("review", review_mcp_at(root.path()))
        .on_url_elicitation(move |url, token| {
            let body = browser.clone();
            async move { submit_in_browser(&url, &token, body).await }
        })
        .elicitation_response(ElicitResult::new(ElicitationAction::Accept));
    let page = json!({
        "format": "html",
        "title": "Landing page",
        "source": {"type": "content", "content": "<main><h1>Ship faster</h1></main>"}
    });

    let run = call_tool(agent, REVIEW_ARTIFACT, page).await;

    let output: serde_json::Value = noyalib::from_str(&run.result().expect("review completes").result).unwrap();
    assert_eq!(output, submission);
    assert_eq!(run.elicitations.len(), 1);
}

async fn submit_in_browser(url: &str, token: &str, body: serde_json::Value) {
    reqwest::Client::new()
        .post(format!("{url}submit?token={token}"))
        .json(&body)
        .send()
        .await
        .expect("submit review")
        .error_for_status()
        .expect("submit accepted");
}

fn review_mcp_at(root: &Path) -> ReviewMcp {
    ReviewMcp::new(root.to_path_buf())
}

fn markdown_plan() -> serde_json::Value {
    json!({"format": "markdown", "source": {"type": "file", "path": "plan.md"}})
}

const REVIEW_ARTIFACT: &str = "review__review_artifact";
