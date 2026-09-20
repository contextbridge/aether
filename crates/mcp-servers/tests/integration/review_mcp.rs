use crate::common::{TestClient, TestResult, scripted_mcp_client, silent_mcp_client, test_error};
use axum::{Router, extract::Query, http::header::CONTENT_TYPE, response::Html, routing::get};
use mcp_servers::review::ReviewMcp;
use mcp_utils::{client::McpClient, testing::ElicitationScript};
use reqwest::Url;
use rmcp::model::{
    CallToolRequest, CallToolRequestParams, CallToolResponse, ClientRequest, ElicitRequestParams, ElicitResult,
    ElicitationAction, InputRequest, InputResponses, RequestMetaObject,
};
use rmcp::service::PeerRequestOptions;
use serde_json::{Value, json};
use std::{collections::HashMap, fs, path::Path, path::PathBuf};
use tempfile::TempDir;
use tokio::task::JoinHandle;
use utils::artifact_review::ArtifactReviewElicitationMeta;

#[tokio::test]
async fn reviews_relative_markdown_file_and_returns_feedback_verbatim() -> TestResult {
    let markdown = "# Title\n\n- A\n- B\n";
    let feedback = "# Markdown review feedback\n\n## `docs/question.md`\n\n### Line 3 — List item\n\n```markdown\n- A\n```\n\n> Choose A.";
    let test = review_test().file("docs/question.md", markdown).build().await?;

    let request = test.request_form(markdown_file("docs/question.md")).await?;
    let expected_path = test.path("docs/question.md");
    assert_eq!(request.meta.path.as_deref(), Some(expected_path.as_path()));
    assert_eq!(request.meta.markdown, markdown);
    assert_eq!(request.meta.title, format!("Review {}", expected_path.display()));
    assert_eq!(request.message, format!("Review {} and approve or submit feedback.", expected_path.display()));
    assert_eq!(request.schema["required"], json!(["decision"]));
    assert_eq!(request.schema["properties"]["decision"]["enum"], json!(["approved", "feedback"]));
    assert!(request.schema["properties"].get("feedback").is_some());
    assert!(request.schema["properties"].get("annotations").is_none());

    test.remove("docs/question.md")?;
    assert_eq!(
        test.submit(markdown_file("docs/question.md"), None, feedback_response(feedback)).await?,
        json!({"status": "feedback", "feedback": feedback})
    );
    Ok(())
}

#[tokio::test]
async fn reviews_inline_markdown_without_creating_a_file() -> TestResult {
    let test = review_test().responds_with(approved_response()).build().await?;

    assert_eq!(
        test.review(markdown_content("# Pick one\n\n- A\n- B", Some("Database choice"))).await?,
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
    let request = test.request_form(markdown_content("# Question", None)).await?;

    assert_eq!(request.meta.path, None);
    assert_eq!(request.meta.title, "Review");
    assert_eq!(request.meta.markdown, "# Question");
    assert_eq!(request.message, "Review this content and approve or submit feedback.");
    Ok(())
}

#[tokio::test]
async fn malformed_inputs_are_rejected() -> TestResult {
    let test = review_test().file("valid.md", "# Valid").build().await?;
    for args in [
        json!({"source": {"type": "file", "path": "valid.md"}}),
        json!({"format": "markdown", "source": {"type": "url", "url": "http://127.0.0.1:1/"}}),
    ] {
        let result = test.client.call_raw("review_artifact", args.clone()).await?;
        assert_eq!(result.is_error, Some(true), "{args} must be rejected: {result:?}");
    }
    Ok(())
}

#[tokio::test]
async fn html_content_request_emits_a_url_elicitation_and_serves_the_artifact() -> TestResult {
    let test = review_test().build().await?;
    let html = "<main><h1>Ship faster</h1></main>";
    let review = test.request_url(html_content(html, Some("Landing page"))).await?;

    assert!(review.url.starts_with("http://127.0.0.1:"), "loopback url: {}", review.url);
    assert!(review.message.contains("Landing page"), "{}", review.message);

    let document = reqwest::get(&review.url).await?.error_for_status()?.text().await?;
    assert!(document.contains(html), "the artifact is served verbatim");
    assert!(document.contains(&format!("data-token=\"{}\"", review.token)), "the overlay carries the submit token");
    assert!(document.contains("attachShadow"), "the overlay script is appended");
    Ok(())
}

#[tokio::test]
async fn html_feedback_round_trips_annotations_through_the_browser_post() -> TestResult {
    let test = review_test().build().await?;
    let args = html_content("<main><h1>Ship faster</h1></main>", Some("Landing page"));
    let review = test.request_url(args.clone()).await?;

    let submission = json!({
        "status": "feedback",
        "feedback": "Tighten the hero.",
        "annotations": [{"element": "<h1>", "excerpt": "Ship faster", "comment": "Make this larger on mobile."}]
    });
    review.submit(&submission).await?.error_for_status()?;

    let output = test.submit(args, Some(&review.token), ElicitResult::new(ElicitationAction::Accept)).await?;
    assert_eq!(output, submission);
    Ok(())
}

#[tokio::test]
async fn html_approval_and_cancel_round_trip_through_the_browser_post() -> TestResult {
    for status in ["approved", "cancelled"] {
        let test = review_test().build().await?;
        let args = html_content("<h1>Hi</h1>", None);
        let review = test.request_url(args.clone()).await?;

        review.submit(&json!({"status": status})).await?.error_for_status()?;

        let output = test.submit(args, Some(&review.token), ElicitResult::new(ElicitationAction::Accept)).await?;
        assert_eq!(output, json!({"status": status}));
    }
    Ok(())
}

#[tokio::test]
async fn html_url_elicitation_cancelled_in_the_terminal_returns_cancelled() -> TestResult {
    let test = review_test().build().await?;
    let args = html_content("<h1>Hi</h1>", None);
    let review = test.request_url(args.clone()).await?;

    let output = test.submit(args, Some(&review.token), ElicitResult::new(ElicitationAction::Cancel)).await?;
    assert_eq!(output, json!({"status": "cancelled"}));
    Ok(())
}

#[tokio::test]
async fn cancelling_the_tool_call_tears_down_the_review_server() -> TestResult {
    let test = review_test().build().await?;
    let args = html_content("<h1>Hi</h1>", None);
    let review = test.request_url(args.clone()).await?;
    assert!(reqwest::get(&review.url).await?.status().is_success());

    let second_round =
        response_request(args, ElicitResult::new(ElicitationAction::Accept)).with_request_state(review.token.clone());
    let request = ClientRequest::CallToolRequest(CallToolRequest::new(second_round));
    let handle = test.client.raw().send_cancellable_request(request, PeerRequestOptions::no_options()).await?;
    handle.cancel(None).await?;

    while reqwest::get(&review.url).await.is_ok() {
        tokio::task::yield_now().await;
    }
    Ok(())
}

#[tokio::test]
async fn a_submit_with_the_wrong_token_is_rejected() -> TestResult {
    let test = review_test().build().await?;
    let review = test.request_url(html_content("<h1>Hi</h1>", None)).await?;

    let response =
        reqwest::Client::new().post(review.submit_url("wrong")).json(&json!({"status": "approved"})).send().await?;
    assert_eq!(response.status(), 403);
    Ok(())
}

#[tokio::test]
async fn html_file_source_serves_the_document_and_its_sibling_assets() -> TestResult {
    let html = "<section><h2>Pricing</h2><img src=\"logo.svg\"></section>";
    let test = review_test().file("site/pricing.html", html).file("site/logo.svg", "<svg></svg>").build().await?;
    let review = test.request_url(html_file(test.path("site/pricing.html"))).await?;

    let document = reqwest::get(&review.url).await?.error_for_status()?.text().await?;
    assert!(document.contains(html), "the artifact is served verbatim");
    assert!(document.contains("attachShadow"), "the overlay is appended");

    let logo = reqwest::get(review.origin().join("logo.svg")?).await?.error_for_status()?;
    assert_eq!(logo.headers()[CONTENT_TYPE], "image/svg+xml");
    assert_eq!(logo.text().await?, "<svg></svg>", "relative URLs resolve against the file's directory");
    Ok(())
}

#[tokio::test]
async fn html_url_source_proxies_the_app_and_injects_the_overlay() -> TestResult {
    let app = upstream_app().await?;
    let test = review_test().build().await?;
    let args = html_url(&format!("{}/dashboard?tab=billing", app.origin));
    let review = test.request_url(args.clone()).await?;
    assert!(review.url.ends_with("/dashboard?tab=billing"), "the app's path and query are kept: {}", review.url);

    let page = reqwest::get(&review.url).await?.error_for_status()?.text().await?;
    assert!(page.contains("<h1>Dashboard: billing</h1>"), "the app's page is proxied: {page}");
    assert!(page.contains("attachShadow"), "the overlay is injected into the app's HTML");

    let script = reqwest::get(review.origin().join("/app.js")?).await?.error_for_status()?;
    assert_eq!(script.headers()[CONTENT_TYPE], "text/javascript");
    assert_eq!(script.text().await?, "console.log('app')", "non-HTML responses pass through untouched");

    review.submit(&json!({"status": "approved"})).await?.error_for_status()?;
    let output = test.submit(args, Some(&review.token), ElicitResult::new(ElicitationAction::Accept)).await?;
    assert_eq!(output, json!({"status": "approved"}));
    Ok(())
}

#[tokio::test]
async fn submitting_without_feedback_is_not_an_approval_decision() -> TestResult {
    let test = review_test().build().await?;
    for content in [json!({"decision": "feedback", "feedback": ""}), json!({"decision": "feedback"})] {
        assert_eq!(
            test.submit(markdown_file("deleted.md"), None, accepted_response(content)).await?,
            json!({"status": "feedback", "feedback": ""})
        );
    }
    Ok(())
}

#[tokio::test]
async fn explicit_approval_returns_approved_without_feedback() -> TestResult {
    let test = review_test().build().await?;
    for content in [json!({"decision": "approved"}), json!({"decision": "approved", "feedback": ""})] {
        assert_eq!(
            test.submit(markdown_file("deleted.md"), None, accepted_response(content)).await?,
            json!({"status": "approved"})
        );
    }
    Ok(())
}

#[tokio::test]
async fn clients_without_elicitation_support_are_rejected() -> TestResult {
    let root = TempDir::new()?;
    fs::write(root.path().join("valid.md"), "# Valid")?;
    let mcp = TestClient::start(|| review_mcp_at(root.path())).await?;

    let result = mcp.call_raw("review_artifact", markdown_file("valid.md")).await?;

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

/// The form elicitation a Markdown review sends to the terminal.
struct FormReview {
    meta: ArtifactReviewElicitationMeta,
    message: String,
    schema: Value,
}

/// The URL elicitation an HTML review sends, plus the loopback server behind it.
struct UrlReview {
    url: String,
    message: String,
    token: String,
}

struct UpstreamApp {
    origin: String,
    server: JoinHandle<()>,
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

    /// The first MRTR round of a Markdown review.
    async fn request_form(&self, args: Value) -> TestResult<FormReview> {
        FormReview::from_response(self.client.raw().call_tool_once(tool_request(args)).await?)
    }

    /// The first MRTR round of an HTML review.
    async fn request_url(&self, args: Value) -> TestResult<UrlReview> {
        UrlReview::from_response(self.client.raw().call_tool_once(tool_request(args)).await?)
    }

    /// The second MRTR round: the client echoes the arguments, the review token
    /// it was handed (HTML only), and the user's elicitation answer.
    async fn submit(&self, args: Value, token: Option<&str>, response: ElicitResult) -> TestResult<Value> {
        let mut request = response_request(args, response);
        if let Some(token) = token {
            request = request.with_request_state(token.to_string());
        }
        structured_output(self.client.raw().call_tool_once(request).await?)
    }

    /// A full review driven by the scripted elicitation response.
    async fn review(&self, args: Value) -> TestResult<Value> {
        self.client.call("review_artifact", args).await
    }
}

impl FormReview {
    fn from_response(response: CallToolResponse) -> TestResult<Self> {
        let request = elicitation(response)?;
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

impl UrlReview {
    fn from_response(response: CallToolResponse) -> TestResult<Self> {
        let CallToolResponse::InputRequired(required) = &response else {
            return Err(test_error(format!("expected input required: {response:?}")).into());
        };
        let token = required.request_state.clone().ok_or_else(|| test_error("expected request state"))?;
        let request = elicitation(response)?;
        let ElicitRequestParams::UrlElicitationParams { message, url, elicitation_id, .. } = &request.params else {
            return Err(test_error("expected url elicitation").into());
        };
        assert_eq!(elicitation_id, &token, "the request state and the elicitation id are the same review token");
        Ok(Self { url: url.clone(), message: message.clone(), token })
    }

    fn origin(&self) -> Url {
        Url::parse(&self.url).expect("review url").join("/").expect("origin")
    }

    fn submit_url(&self, token: &str) -> Url {
        let mut url = self.origin().join("submit").expect("submit url");
        url.set_query(Some(&format!("token={token}")));
        url
    }

    async fn submit(&self, body: &Value) -> reqwest::Result<reqwest::Response> {
        reqwest::Client::new().post(self.submit_url(&self.token)).json(body).send().await
    }
}

fn elicitation(response: CallToolResponse) -> TestResult<rmcp::model::ElicitRequest> {
    let CallToolResponse::InputRequired(required) = response else {
        return Err(test_error(format!("expected input required: {response:?}")).into());
    };
    let mut requests = required.input_requests.ok_or_else(|| test_error("expected input requests"))?;
    match requests.remove("review") {
        Some(InputRequest::Elicitation(request)) => Ok(request),
        other => Err(test_error(format!("expected review elicitation: {other:?}")).into()),
    }
}

async fn upstream_app() -> TestResult<UpstreamApp> {
    let router = Router::new()
        .route(
            "/dashboard",
            get(|Query(query): Query<HashMap<String, String>>| async move {
                Html(format!(
                    "<html><body><h1>Dashboard: {}</h1><script src=\"/app.js\"></script></body></html>",
                    query["tab"]
                ))
            }),
        )
        .route("/app.js", get(|| async { ([(CONTENT_TYPE, "text/javascript")], "console.log('app')") }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let origin = format!("http://{}", listener.local_addr()?);
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    Ok(UpstreamApp { origin, server })
}

impl Drop for UpstreamApp {
    fn drop(&mut self) {
        self.server.abort();
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

fn markdown_file(path: impl AsRef<Path>) -> Value {
    json!({"format": "markdown", "source": {"type": "file", "path": path.as_ref()}})
}

fn markdown_content(content: &str, title: Option<&str>) -> Value {
    with_title(json!({"format": "markdown", "source": {"type": "content", "content": content}}), title)
}

fn html_file(path: impl AsRef<Path>) -> Value {
    json!({"format": "html", "source": {"type": "file", "path": path.as_ref()}})
}

fn html_content(content: &str, title: Option<&str>) -> Value {
    with_title(json!({"format": "html", "source": {"type": "content", "content": content}}), title)
}

fn html_url(url: &str) -> Value {
    json!({"format": "html", "source": {"type": "url", "url": url}})
}

fn with_title(mut args: Value, title: Option<&str>) -> Value {
    if let Some(title) = title {
        args["title"] = json!(title);
    }
    args
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
