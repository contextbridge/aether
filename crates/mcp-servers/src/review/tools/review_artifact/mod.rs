use crate::workspace_paths::resolve_path;
use mcp_utils::server::mrtr::{ELICITATION_UNSUPPORTED, input_requests_supported, parse_response};
use rmcp::{
    ErrorData as McpError, RoleServer,
    handler::server::{
        tool::{InputResponses, IntoCallToolResult},
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
pub use utils::artifact_review::ArtifactFormat;
use utils::artifact_review::{ArtifactReviewDecision, ArtifactReviewElicitationMeta, ArtifactReviewSubmission};

const REVIEW: &str = "review";

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewArtifactInput {
    pub source: ReviewArtifactSource,
    pub title: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum ReviewArtifactSource {
    File { path: PathBuf },
    Content { content: String },
}

#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum ReviewArtifactOutput {
    Approved,
    Feedback { feedback: String },
    Cancelled,
    Declined,
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
                    .ok_or_else(|| McpError::invalid_params("accepted review response is missing content", None))?;
                let submission: ArtifactReviewSubmission = serde_json::from_value(content).map_err(|error| {
                    McpError::invalid_params(format!("invalid accepted review response: {error}"), None)
                })?;

                Ok(match submission {
                    ArtifactReviewSubmission::Approved => Self::Approved,
                    ArtifactReviewSubmission::Feedback { feedback } => Self::Feedback { feedback },
                })
            }
            _ => Err(McpError::invalid_params("unknown elicitation action", None)),
        }
    }
}

pub async fn execute_review_artifact(
    root_dir: &Path,
    input: ReviewArtifactInput,
    responses: InputResponses,
    context: &RequestContext<RoleServer>,
) -> Result<CallToolResponse, McpError> {
    if let Some(responses) = responses.0 {
        let result: ElicitResult = parse_response(&responses, REVIEW)?;
        return Json(ReviewArtifactOutput::try_from(result)?).into_call_tool_result();
    }

    let (meta, subject) = match input.source {
        ReviewArtifactSource::File { path } => {
            let path = resolve_path(root_dir, path);
            let markdown = match read_file(&path).await {
                Ok(markdown) => markdown,
                Err(error) => return Ok(CallToolResult::error(vec![ContentBlock::text(error)]).into()),
            };

            let subject = input.title.clone().unwrap_or_else(|| path.display().to_string());
            let meta = ArtifactReviewElicitationMeta::new(&path, &markdown, ArtifactFormat::Markdown)
                .with_title(input.title.unwrap_or_else(|| format!("Review {}", path.display())));
            (meta, subject)
        }

        ReviewArtifactSource::Content { content: inline_markdown } => {
            let (title, subject) = input
                .title
                .map_or_else(|| ("Review".to_string(), "this content".to_string()), |title| (title.clone(), title));
            (ArtifactReviewElicitationMeta::inline(&title, &inline_markdown, ArtifactFormat::Markdown), subject)
        }
    };

    let params = build_elicitation_form(&meta, &subject)?;
    let requests = InputRequests::from([(REVIEW.to_string(), InputRequest::Elicitation(ElicitRequest::new(params)))]);

    if !input_requests_supported(context.client_capabilities().as_ref(), &requests) {
        return Ok(CallToolResult::error(vec![ContentBlock::text(ELICITATION_UNSUPPORTED)]).into());
    }

    Ok(InputRequiredResult::from_input_requests(requests).into())
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
) -> Result<ElicitRequestParams, McpError> {
    let meta = artifact.to_json().map(RequestMetaObject::from).map_err(|error| {
        McpError::internal_error(format!("failed to serialize artifact review metadata: {error}"), None)
    })?;

    let requested_schema = ElicitationSchema::builder()
        .required_enum_schema(
            "decision",
            EnumSchema::builder(ArtifactReviewDecision::ALL.map(|decision| decision.to_string()).to_vec()).build(),
        )
        .optional_string("feedback")
        .build()
        .map_err(|error| McpError::internal_error(format!("failed to build schema: {error}"), None))?;

    Ok(ElicitRequestParams::FormElicitationParams {
        meta: Some(meta),
        message: format!("Review {subject} and approve or submit feedback."),
        requested_schema,
    })
}
