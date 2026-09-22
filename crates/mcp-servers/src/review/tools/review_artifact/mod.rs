use crate::review::html_review::{Artifact, PendingReviews, ReviewServer};
use crate::workspace_paths::resolve_path;
use mcp_utils::server::mrtr::{ELICITATION_UNSUPPORTED, ElicitationMode, elicitation_supported, parse_response};
use reqwest::Url;
use rmcp::{
    ErrorData as McpError, RoleServer,
    handler::server::{
        tool::{InputResponses, IntoCallToolResult, RequestState},
        wrapper::Json,
    },
    model::{
        CallToolResponse, CallToolResult, ContentBlock, ElicitRequest, ElicitRequestParams, ElicitResult,
        ElicitationAction, ElicitationSchema, EnumSchema, InputRequest, InputRequests, InputRequiredResult,
        RequestMetaObject,
    },
    service::RequestContext,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use utils::artifact_review::{ArtifactReviewDecision, ArtifactReviewElicitationMeta, ArtifactReviewSubmission};

const REVIEW: &str = "review";

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "format", rename_all = "lowercase", deny_unknown_fields)]
#[schemars(inline, extend("type" = "object"))]
pub enum ReviewArtifactInput {
    Markdown {
        source: DocumentSource,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    Html {
        source: HtmlSource,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
#[schemars(inline)]
pub enum DocumentSource {
    File { path: PathBuf },
    Content { content: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
#[schemars(inline)]
pub enum HtmlSource {
    File { path: PathBuf },
    Content { content: String },
    Url { url: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum ReviewArtifactOutput {
    Approved {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
    },
    Feedback {
        feedback: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        annotations: Vec<HtmlAnnotation>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
    },
    Cancelled,
    Declined,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HtmlAnnotation {
    pub element: String,
    pub excerpt: String,
    pub comment: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

pub struct ReviewArtifactTool {
    root_dir: PathBuf,
    pending: PendingReviews,
}

impl ReviewArtifactTool {
    pub fn new(root_dir: PathBuf) -> Self {
        Self { root_dir, pending: PendingReviews::default() }
    }

    pub async fn execute(
        &self,
        input: ReviewArtifactInput,
        responses: InputResponses,
        request_state: RequestState,
        context: &RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        match responses.0 {
            Some(responses) => self.respond(&responses, request_state, context).await,
            None => Ok(self.request(input, context).await.unwrap_or_else(tool_error)),
        }
    }

    async fn request(
        &self,
        input: ReviewArtifactInput,
        context: &RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, String> {
        let mode = match &input {
            ReviewArtifactInput::Markdown { .. } => ElicitationMode::Form,
            ReviewArtifactInput::Html { .. } => ElicitationMode::Url,
        };
        if !elicitation_supported(context.client_capabilities().as_ref(), mode) {
            return Err(ELICITATION_UNSUPPORTED.to_string());
        }

        let (params, review_token) = match input {
            ReviewArtifactInput::Markdown { source, title } => {
                let (markdown, path) = self.load_document(source).await?;
                let label = path.as_ref().map(|path| path.display().to_string());
                let meta = ArtifactReviewElicitationMeta::new(
                    path,
                    review_title(title.as_deref(), label.as_deref()),
                    markdown,
                );
                (build_elicitation_form(&meta, &review_subject(title.as_deref(), label.as_deref()))?, None)
            }
            ReviewArtifactInput::Html { source, title } => {
                let (artifact, label) = self.load_html(source).await?;
                let subject = review_subject(title.as_deref(), label.as_deref());
                let server = ReviewServer::start(artifact)
                    .map_err(|error| format!("Failed to start the review server: {error}"))?;
                let token = server.token().to_string();
                let params = ElicitRequestParams::UrlElicitationParams {
                    meta: None,
                    message: format!("Review {subject} in your browser, then submit or cancel there."),
                    url: server.url().to_string(),
                    elicitation_id: token.clone(),
                };
                self.pending.insert(server);
                (params, Some(token))
            }
        };

        let requests =
            InputRequests::from([(REVIEW.to_string(), InputRequest::Elicitation(ElicitRequest::new(params)))]);
        Ok(InputRequiredResult::new(Some(requests), review_token).into())
    }

    async fn respond(
        &self,
        responses: &rmcp::model::InputResponses,
        request_state: RequestState,
        context: &RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let result: ElicitResult = parse_response(responses, REVIEW)?;
        let output = match request_state.0 {
            None => ReviewArtifactOutput::try_from(result)?,
            Some(token) => {
                let review = self.pending.take(&token);
                match result.action {
                    ElicitationAction::Accept => {
                        let review = review.ok_or_else(|| {
                            McpError::invalid_params("the HTML review expired before it was submitted", None)
                        })?;
                        review.wait(context.ct.clone()).await
                    }
                    ElicitationAction::Cancel => ReviewArtifactOutput::Cancelled,
                    ElicitationAction::Decline => ReviewArtifactOutput::Declined,
                    _ => return Err(McpError::invalid_params("unknown elicitation action", None)),
                }
            }
        };
        Json(output).into_call_tool_result()
    }

    async fn load_document(&self, source: DocumentSource) -> Result<(String, Option<PathBuf>), String> {
        match source {
            DocumentSource::File { path } => {
                let path = resolve_path(&self.root_dir, path);
                Ok((read_file(&path).await?, Some(path)))
            }
            DocumentSource::Content { content } => Ok((content, None)),
        }
    }

    async fn load_html(&self, source: HtmlSource) -> Result<(Artifact, Option<String>), String> {
        match source {
            HtmlSource::File { path } => {
                let path = resolve_path(&self.root_dir, path);
                let html = read_file(&path).await?;
                let assets = path.parent().map(Path::to_path_buf);
                Ok((Artifact::Document { html, assets }, Some(path.display().to_string())))
            }
            HtmlSource::Content { content } => Ok((Artifact::Document { html: content, assets: None }, None)),
            HtmlSource::Url { url } => {
                let parsed = Url::parse(&url).map_err(|error| format!("Invalid artifact URL {url}: {error}"))?;
                if !matches!(parsed.scheme(), "http" | "https") {
                    return Err(format!("Artifact URL must use http or https: {url}"));
                }
                Ok((Artifact::App(parsed), Some(url)))
            }
        }
    }
}

impl TryFrom<ElicitResult> for ReviewArtifactOutput {
    type Error = McpError;

    fn try_from(result: ElicitResult) -> Result<Self, Self::Error> {
        match result.action {
            ElicitationAction::Cancel => Ok(Self::Cancelled),
            ElicitationAction::Decline => Ok(Self::Declined),
            ElicitationAction::Accept => {
                let content = result
                    .content
                    .ok_or_else(|| McpError::invalid_params("accepted review response must include content", None))?;
                let submission: ArtifactReviewSubmission = serde_json::from_value(content).map_err(|error| {
                    McpError::invalid_params(format!("invalid accepted review response: {error}"), None)
                })?;
                Ok(match submission {
                    ArtifactReviewSubmission::Approved => Self::Approved { url: None },
                    ArtifactReviewSubmission::Feedback { feedback } => {
                        Self::Feedback { feedback, annotations: Vec::new(), url: None }
                    }
                })
            }
            _ => Err(McpError::invalid_params("unknown elicitation action", None)),
        }
    }
}

fn review_title(title: Option<&str>, label: Option<&str>) -> String {
    match (title, label) {
        (Some(title), _) => title.to_string(),
        (None, Some(label)) => format!("Review {label}"),
        (None, None) => "Review".to_string(),
    }
}

fn review_subject(title: Option<&str>, label: Option<&str>) -> String {
    title.or(label).map_or_else(|| "this content".to_string(), str::to_string)
}

fn tool_error(message: String) -> CallToolResponse {
    CallToolResult::error(vec![ContentBlock::text(message)]).into()
}

async fn read_file(path: &Path) -> Result<String, String> {
    let metadata = tokio::fs::metadata(path)
        .await
        .map_err(|error| format!("Failed to read artifact {}: {error}", path.display()))?;

    if !metadata.is_file() {
        return Err(format!("Artifact is not a regular file: {}", path.display()));
    }

    tokio::fs::read_to_string(path)
        .await
        .map_err(|error| format!("Failed to read UTF-8 artifact {}: {error}", path.display()))
}

fn build_elicitation_form(
    artifact: &ArtifactReviewElicitationMeta,
    subject: &str,
) -> Result<ElicitRequestParams, String> {
    let meta = artifact
        .to_json()
        .map(RequestMetaObject::from)
        .map_err(|error| format!("Failed to serialize artifact review metadata: {error}"))?;

    let requested_schema = ElicitationSchema::builder()
        .required_enum_schema(
            "decision",
            EnumSchema::builder(ArtifactReviewDecision::ALL.map(|decision| decision.to_string()).to_vec()).build(),
        )
        .optional_string("feedback")
        .build()
        .map_err(|error| format!("Failed to build the review form schema: {error}"))?;

    Ok(ElicitRequestParams::FormElicitationParams {
        meta: Some(meta),
        message: format!("Review {subject} and approve or submit feedback."),
        requested_schema,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_schema_is_a_self_contained_object() {
        let schema = serde_json::to_value(schemars::schema_for!(ReviewArtifactInput)).expect("schema serializes");
        let text = schema.to_string();
        assert!(
            !text.contains("$ref"),
            "refs must be inlined so providers that ignore $defs still see the shape: {text}"
        );
        assert_eq!(schema["type"], "object", "providers require an object at the root: {text}");
    }

    #[test]
    fn markdown_cannot_be_reviewed_from_a_url() {
        let input = serde_json::json!({"format": "markdown", "source": {"type": "url", "url": "http://127.0.0.1:1/"}});
        assert!(serde_json::from_value::<ReviewArtifactInput>(input).is_err());
    }
}
