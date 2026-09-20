use crate::common::{TestClient, TestResult, scripted_mcp_client, silent_mcp_client, test_error};
use mcp_servers::review::{ArtifactFormat, ReviewMcp};
use mcp_utils::{client::McpClient, testing::ElicitationScript};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, ElicitRequestParams, ElicitResult, ElicitationAction, InputRequest,
    InputResponses, RequestMetaObject,
};
use serde_json::{Value, json};
use std::{fs, path::Path, path::PathBuf};
use tempfile::TempDir;
use utils::artifact_review::ArtifactReviewElicitationMeta;

#[tokio::test]
async fn reviews_relative_markdown_file_and_returns_feedback_verbatim() -> TestResult {
    let markdown = "# Pick one\n\n- A\n- B";
    let feedback = "# Markdown review feedback\n\n## `docs/question.md`\n\n### Line 3 — List item\n\n```markdown\n- A\n```\n\n> Choose A.";
    let test = review_test().file("docs/question.md", markdown).build().await?;

    let request = test.request_file("docs/question.md").await?;
    let expected_path = test.path("docs/question.md");
    assert_eq!(request.meta.path.as_deref(), Some(expected_path.as_path()));
    assert_eq!(request.meta.markdown, markdown);
    assert_eq!(request.meta.format, ArtifactFormat::Markdown);
    assert_eq!(request.schema["required"], json!(["decision"]));
    assert_eq!(request.schema["properties"]["decision"]["enum"], json!(["approved", "feedback"]));
    assert!(request.schema["properties"].get("feedback").is_some());

    test.remove("docs/question.md")?;
    assert_eq!(
        test.submit_file("docs/question.md", feedback_response(feedback)).await?,
        json!({"status": "feedback", "feedback": feedback})
    );
    Ok(())
}

#[tokio::test]
async fn reviews_inline_markdown_without_creating_a_file() -> TestResult {
    let test = review_test().responds_with(approved_response()).build().await?;

    assert_eq!(
        test.review_content("# Pick one\n\n- A\n- B", Some("Database choice")).await?,
        json!({"status": "approved"})
    );
    let captured = test.script.as_ref().expect("scripted review client").captured();
    let ElicitRequestParams::FormElicitationParams { meta: Some(meta), .. } = &captured[0].request else {
        return Err(test_error("expected artifact metadata in the review UI request").into());
    };
    let meta = ArtifactReviewElicitationMeta::parse(Some(&meta.0))
        .ok_or_else(|| test_error("invalid artifact metadata in the review UI request"))?;
    assert_eq!(meta.title, "Database choice");
    assert_eq!(meta.markdown, "# Pick one\n\n- A\n- B");
    assert!(test.root_is_empty()?);
    Ok(())
}

#[tokio::test]
async fn inline_markdown_is_sent_directly_to_the_review_ui() -> TestResult {
    let test = review_test().build().await?;
    let request = test.request_content("# Question", None).await?;

    assert_eq!(request.meta.path, None);
    assert_eq!(request.meta.title, "Review");
    assert_eq!(request.meta.markdown, "# Question");
    assert_eq!(request.message, "Review this content and approve or submit feedback.");
    Ok(())
}

#[tokio::test]
async fn submitting_without_feedback_is_not_an_approval_decision() -> TestResult {
    let test = review_test().build().await?;
    for content in [json!({"decision": "feedback", "feedback": ""}), json!({"decision": "feedback"})] {
        assert_eq!(
            test.submit_file("deleted.md", accepted_response(content)).await?,
            json!({"status": "feedback", "feedback": ""})
        );
    }
    Ok(())
}

#[tokio::test]
async fn explicit_approval_returns_approved_without_feedback() -> TestResult {
    let test = review_test().build().await?;
    for content in [json!({"decision": "approved"}), json!({"decision": "approved", "feedback": ""})] {
        assert_eq!(test.submit_file("deleted.md", accepted_response(content)).await?, json!({"status": "approved"}));
    }
    Ok(())
}

#[tokio::test]
async fn clients_without_elicitation_support_are_rejected() -> TestResult {
    let root = TempDir::new()?;
    fs::write(root.path().join("valid.md"), "# Valid")?;
    let mcp = TestClient::start(|| review_mcp_at(root.path())).await?;

    let result = mcp.call_raw("review_artifact", json!({"source": {"type": "file", "path": "valid.md"}})).await?;

    assert_eq!(result.is_error, Some(true));
    Ok(())
}

fn review_test() -> ReviewTestBuilder {
    ReviewTestBuilder::default()
}

#[derive(Default)]
struct ReviewTestBuilder {
    files: Vec<(PathBuf, Vec<u8>)>,
    response: Option<ElicitResult>,
}

struct ReviewTest {
    root: TempDir,
    client: TestClient<ReviewMcp, McpClient>,
    script: Option<ElicitationScript>,
}

struct ReviewRequest {
    meta: ArtifactReviewElicitationMeta,
    message: String,
    schema: Value,
}

impl ReviewTestBuilder {
    fn file(mut self, path: impl Into<PathBuf>, content: impl AsRef<[u8]>) -> Self {
        self.files.push((path.into(), content.as_ref().to_vec()));
        self
    }

    fn responds_with(mut self, response: ElicitResult) -> Self {
        self.response = Some(response);
        self
    }

    async fn build(self) -> TestResult<ReviewTest> {
        let root = TempDir::new()?;
        for (relative_path, content) in self.files {
            let path = root.path().join(relative_path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(path, content)?;
        }

        let (client, script) = self.response.map_or_else(
            || (silent_mcp_client("review-test-server"), None),
            |response| {
                let (client, script) = scripted_mcp_client("review-test-server", response);
                (client, Some(script))
            },
        );
        let client = TestClient::start_with(|| review_mcp_at(root.path()), client).await?;
        Ok(ReviewTest { root, client, script })
    }
}

impl ReviewTest {
    fn path(&self, relative_path: impl AsRef<Path>) -> PathBuf {
        self.root.path().join(relative_path)
    }

    fn remove(&self, relative_path: impl AsRef<Path>) -> TestResult {
        fs::remove_file(self.path(relative_path))?;
        Ok(())
    }

    fn root_is_empty(&self) -> TestResult<bool> {
        Ok(fs::read_dir(self.root.path())?.next().is_none())
    }

    async fn request_file(&self, path: impl AsRef<Path>) -> TestResult<ReviewRequest> {
        self.request(json!({"source": {"type": "file", "path": path.as_ref()}})).await
    }

    async fn request_content(&self, content: &str, title: Option<&str>) -> TestResult<ReviewRequest> {
        self.request(content_args(content, title)).await
    }

    async fn request(&self, args: Value) -> TestResult<ReviewRequest> {
        let response = self.client.raw().call_tool_once(tool_request(args)).await?;
        ReviewRequest::from_response(response)
    }

    async fn submit_file(&self, path: impl AsRef<Path>, response: ElicitResult) -> TestResult<Value> {
        let args = json!({"source": {"type": "file", "path": path.as_ref()}});
        let response = self.client.raw().call_tool_once(response_request(args, response)).await?;
        structured_output(response)
    }

    async fn review_content(&self, content: &str, title: Option<&str>) -> TestResult<Value> {
        self.client.call("review_artifact", content_args(content, title)).await
    }
}

impl ReviewRequest {
    fn from_response(response: CallToolResponse) -> TestResult<Self> {
        let CallToolResponse::InputRequired(required) = response else {
            return Err(test_error(format!("expected input required: {response:?}")).into());
        };
        let requests = required.input_requests.ok_or_else(|| test_error("expected input requests"))?;
        let InputRequest::Elicitation(request) = requests.get("review").ok_or_else(|| test_error("review request"))?
        else {
            return Err(test_error("expected review elicitation").into());
        };
        let ElicitRequestParams::FormElicitationParams { meta, message, requested_schema } = &request.params else {
            return Err(test_error("expected form elicitation").into());
        };
        let meta = meta
            .as_ref()
            .or_else(|| request.extensions.get::<RequestMetaObject>())
            .ok_or_else(|| test_error("expected artifact metadata"))?;
        let meta = ArtifactReviewElicitationMeta::parse(Some(&meta.0))
            .ok_or_else(|| test_error("invalid artifact metadata"))?;
        Ok(Self { meta, message: message.clone(), schema: serde_json::to_value(requested_schema)? })
    }
}

fn review_mcp_at(root: &Path) -> ReviewMcp {
    ReviewMcp::from_args_with_base_dir(Vec::new(), root).expect("empty review MCP arguments are valid")
}

fn accepted_response(content: Value) -> ElicitResult {
    ElicitResult::new(ElicitationAction::Accept).with_content(content)
}

fn approved_response() -> ElicitResult {
    accepted_response(json!({"decision": "approved"}))
}

fn feedback_response(feedback: &str) -> ElicitResult {
    accepted_response(json!({"decision": "feedback", "feedback": feedback}))
}

fn content_args(content: &str, title: Option<&str>) -> Value {
    title.map_or_else(
        || json!({"source": {"type": "content", "content": content}}),
        |title| json!({"source": {"type": "content", "content": content}, "title": title}),
    )
}

fn tool_request(args: Value) -> CallToolRequestParams {
    let Value::Object(arguments) = args else { panic!("tool arguments") };
    CallToolRequestParams::new("review_artifact").with_arguments(arguments)
}

fn response_request(args: Value, result: ElicitResult) -> CallToolRequestParams {
    tool_request(args).with_input_responses(InputResponses::from_iter([(
        "review".into(),
        serde_json::to_value(result).expect("elicitation result serializes"),
    )]))
}

fn structured_output(response: CallToolResponse) -> TestResult<Value> {
    let CallToolResponse::Complete(result) = response else {
        return Err(test_error(format!("expected complete response: {response:?}")).into());
    };
    result.structured_content.ok_or_else(|| test_error("expected structured output").into())
}
