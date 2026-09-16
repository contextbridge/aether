use crate::common::{TestClient, TestResult, scripted_mcp_client, silent_mcp_client};
use mcp_servers::review::{ArtifactFormat, ReviewMcp};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, ElicitRequestParams, ElicitResult, ElicitationAction, InputRequest,
    InputResponses,
};
use serde_json::json;
use std::fs;
use tempfile::TempDir;
use utils::artifact_review::ArtifactReviewElicitationMeta;

#[tokio::test]
async fn reviews_relative_markdown_file_and_returns_feedback_verbatim() -> TestResult {
    let root = TempDir::new()?;
    let path = root.path().join("docs/question.md");
    fs::create_dir_all(path.parent().expect("parent"))?;
    fs::write(&path, "# Pick one\n\n- A\n- B")?;
    let feedback = "# Markdown review feedback\n\n## `docs/question.md`\n\n### Line 3 — List item\n\n```markdown\n- A\n```\n\n> Choose A.";
    let client = silent_mcp_client("review-test-server");
    let mcp = TestClient::start_with(|| ReviewMcp::new().with_root_dir(root.path().to_path_buf()), client).await?;

    let first = mcp
        .raw()
        .call_tool_once(
            CallToolRequestParams::new("review_artifact")
                .with_arguments(json!({"path": "docs/question.md", "format": "markdown"}).as_object().unwrap().clone()),
        )
        .await?;
    let CallToolResponse::InputRequired(required) = first else { panic!("expected input required: {first:?}") };
    let requests = required.input_requests.expect("input requests");
    let InputRequest::Elicitation(request) = requests.get("review").expect("review request") else {
        panic!("elicitation")
    };
    let ElicitRequestParams::FormElicitationParams { meta, requested_schema, .. } = &request.params else {
        panic!("form")
    };
    let schema = serde_json::to_value(requested_schema)?;
    assert_eq!(schema["required"], json!(["decision"]));
    assert_eq!(schema["properties"]["decision"]["enum"], json!(["approved", "feedback"]));
    assert!(schema["properties"].get("feedback").is_some());
    let parsed =
        ArtifactReviewElicitationMeta::parse(Some(&meta.as_ref().expect("meta").0)).expect("artifact metadata");
    assert_eq!(parsed.path, path);
    assert_eq!(parsed.markdown, "# Pick one\n\n- A\n- B");
    assert_eq!(parsed.format, ArtifactFormat::Markdown);

    fs::remove_file(&path)?;
    let mut responses = InputResponses::new();
    responses.insert(
        "review".into(),
        serde_json::to_value(
            ElicitResult::new(ElicitationAction::Accept)
                .with_content(json!({"decision": "feedback", "feedback": feedback})),
        )?,
    );
    let second = mcp
        .raw()
        .call_tool_once(
            CallToolRequestParams::new("review_artifact")
                .with_arguments(json!({"path": "docs/question.md", "format": "markdown"}).as_object().unwrap().clone())
                .with_input_responses(responses),
        )
        .await?;
    let CallToolResponse::Complete(result) = second else { panic!("expected complete: {second:?}") };
    let output = result.structured_content.expect("structured output");
    assert_eq!(output, json!({"status": "feedback", "feedback": feedback}));
    Ok(())
}

#[tokio::test]
async fn exposes_only_review_artifact_with_required_closed_format() -> TestResult {
    let mcp = TestClient::start_with(ReviewMcp::new, silent_mcp_client("review-test-server")).await?;
    let tools = mcp.raw().list_tools(None).await?.tools;
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "review_artifact");
    let schema = &tools[0].input_schema;
    assert_eq!(schema["required"], json!(["path", "format"]));
    let format_schema = schema["properties"]["format"]["$ref"]
        .as_str()
        .and_then(|reference| reference.strip_prefix("#/$defs/"))
        .and_then(|name| schema["$defs"].get(name))
        .unwrap_or(&schema["properties"]["format"]);
    assert_eq!(format_schema["enum"], json!(["markdown"]));
    Ok(())
}

#[tokio::test]
async fn submitting_without_feedback_is_not_an_approval_decision() -> TestResult {
    let mcp = TestClient::start_with(ReviewMcp::new, silent_mcp_client("review-test-server")).await?;
    for content in [json!({"decision": "feedback", "feedback": ""}), json!({"decision": "feedback"})] {
        let response = mcp
            .raw()
            .call_tool_once(response_request(ElicitResult::new(ElicitationAction::Accept).with_content(content)))
            .await?;
        let CallToolResponse::Complete(result) = response else { panic!("expected complete") };
        assert_eq!(result.structured_content.expect("output"), json!({"status": "feedback", "feedback": ""}));
    }
    Ok(())
}

#[tokio::test]
async fn explicit_approval_returns_approved_without_feedback() -> TestResult {
    let mcp = TestClient::start_with(ReviewMcp::new, silent_mcp_client("review-test-server")).await?;
    for content in [json!({"decision": "approved"}), json!({"decision": "approved", "feedback": ""})] {
        let response = mcp
            .raw()
            .call_tool_once(response_request(ElicitResult::new(ElicitationAction::Accept).with_content(content)))
            .await?;
        let CallToolResponse::Complete(result) = response else { panic!("expected complete") };
        assert_eq!(result.structured_content.unwrap(), json!({"status": "approved"}));
    }
    Ok(())
}

#[tokio::test]
async fn rejects_ambiguous_or_malformed_accepted_responses() -> TestResult {
    let mcp = TestClient::start_with(ReviewMcp::new, silent_mcp_client("review-test-server")).await?;
    for content in [
        json!({}),
        json!({"feedback": ""}),
        json!({"decision": "unknown"}),
        json!({"decision": "approved", "feedback": "Do not discard this"}),
        json!({"decision": "feedback", "feedback": 42}),
        json!({"decision": "feedback", "extra": true}),
    ] {
        assert!(
            mcp.raw()
                .call_tool_once(response_request(
                    ElicitResult::new(ElicitationAction::Accept).with_content(content.clone()),
                ))
                .await
                .is_err(),
            "accepted invalid content: {content}"
        );
    }
    assert!(mcp.raw().call_tool_once(response_request(ElicitResult::new(ElicitationAction::Accept))).await.is_err());
    Ok(())
}

#[tokio::test]
async fn review_never_modifies_the_artifact() -> TestResult {
    let root = TempDir::new()?;
    let path = root.path().join("plan.md");
    let markdown = "---\ntitle: Plan\n---\n\n# Plan\n";
    fs::write(&path, markdown)?;
    let (client, _script) = scripted_mcp_client(
        "review-test-server",
        ElicitResult::new(ElicitationAction::Accept).with_content(json!({"decision": "approved"})),
    );
    let mcp = TestClient::start_with(|| ReviewMcp::new().with_root_dir(root.path().to_path_buf()), client).await?;
    assert_eq!(
        mcp.call("review_artifact", json!({"path": "plan.md", "format": "markdown"})).await?,
        json!({"status": "approved"})
    );
    assert_eq!(fs::read_to_string(path)?, markdown);
    Ok(())
}

#[tokio::test]
async fn rejects_unreadable_artifacts_and_unsupported_clients() -> TestResult {
    let root = TempDir::new()?;
    fs::write(root.path().join("invalid.md"), [0xff])?;
    fs::write(root.path().join("valid.md"), "# Valid")?;
    let mcp = TestClient::start_with(
        || ReviewMcp::new().with_root_dir(root.path().to_path_buf()),
        silent_mcp_client("review-test-server"),
    )
    .await?;
    for (path, expected) in [
        ("missing.md", "Failed to read artifact"),
        (".", "not a regular file"),
        ("invalid.md", "Failed to read UTF-8 artifact"),
    ] {
        let result = mcp.call_raw("review_artifact", json!({"path": path, "format": "markdown"})).await?;
        assert_eq!(result.is_error, Some(true));
        assert!(result.content[0].as_text().unwrap().text.contains(expected));
    }
    let unsupported = TestClient::start(|| ReviewMcp::new().with_root_dir(root.path().to_path_buf())).await?;
    let result = unsupported.call_raw("review_artifact", json!({"path": "valid.md", "format": "markdown"})).await?;
    assert_eq!(result.is_error, Some(true));
    for args in [
        json!({"path": "valid.md"}),
        json!({"path": "valid.md", "format": "html"}),
        json!({"path": "valid.md", "format": "markdown", "extra": true}),
    ] {
        let result = mcp.call_raw("review_artifact", args.clone()).await?;
        assert_eq!(result.is_error, Some(true), "invalid arguments accepted: {args}");
        assert!(result.content[0].as_text().unwrap().text.contains("failed to deserialize parameters"));
    }
    Ok(())
}

#[tokio::test]
async fn constructor_roots_preserve_relative_and_absolute_path_semantics() -> TestResult {
    let cwd = std::env::current_dir()?;
    let root = tempfile::tempdir_in(&cwd)?;
    let relative_root = root.path().strip_prefix(&cwd)?.to_path_buf();
    let path = root.path().join("plan.md");
    fs::write(&path, "# Plan")?;
    for (server, input_path, expected_path) in [
        (
            ReviewMcp::from_args(vec!["--root-dir".into(), relative_root.display().to_string()])?,
            "plan.md".into(),
            relative_root.join("plan.md"),
        ),
        (
            ReviewMcp::from_args_with_base_dir(vec!["--root-dir".into(), relative_root.display().to_string()], &cwd)?,
            "plan.md".into(),
            path.clone(),
        ),
        (ReviewMcp::from_args_with_base_dir(vec![], root.path())?, "plan.md".into(), path.clone()),
        (
            ReviewMcp::from_args_with_base_dir(vec!["--root-dir".into(), root.path().display().to_string()], &cwd)?,
            "plan.md".into(),
            path.clone(),
        ),
        (ReviewMcp::new(), path.clone(), path.clone()),
    ] {
        let mcp = TestClient::start_with(|| server, silent_mcp_client("review-test-server")).await?;
        let response = mcp
            .raw()
            .call_tool_once(
                CallToolRequestParams::new("review_artifact")
                    .with_arguments(json!({"path": input_path, "format": "markdown"}).as_object().unwrap().clone()),
            )
            .await?;
        let CallToolResponse::InputRequired(required) = response else { panic!("expected elicitation") };
        let requests = required.input_requests.unwrap();
        let InputRequest::Elicitation(request) = &requests["review"] else { panic!("expected elicitation") };
        let ElicitRequestParams::FormElicitationParams { meta, .. } = &request.params else { panic!("expected form") };
        let meta = ArtifactReviewElicitationMeta::parse(Some(&meta.as_ref().unwrap().0)).unwrap();
        assert_eq!(meta.path, expected_path);
    }
    Ok(())
}

#[tokio::test]
async fn maps_cancel_and_decline_to_distinct_outputs() -> TestResult {
    for (action, status) in [(ElicitationAction::Cancel, "cancelled"), (ElicitationAction::Decline, "declined")] {
        let mcp = TestClient::start_with(ReviewMcp::new, silent_mcp_client("review-test-server")).await?;
        let response = mcp.raw().call_tool_once(response_request(ElicitResult::new(action))).await?;
        let CallToolResponse::Complete(result) = response else { panic!("expected complete") };
        assert_eq!(result.structured_content.expect("output"), json!({"status": status}));
    }
    Ok(())
}

fn response_request(result: ElicitResult) -> CallToolRequestParams {
    CallToolRequestParams::new("review_artifact")
        .with_arguments(json!({"path": "deleted.md", "format": "markdown"}).as_object().unwrap().clone())
        .with_input_responses(InputResponses::from_iter([("review".into(), serde_json::to_value(result).unwrap())]))
}
