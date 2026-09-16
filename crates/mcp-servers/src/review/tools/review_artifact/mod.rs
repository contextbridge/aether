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
use utils::artifact_review::{ArtifactReviewElicitationMeta, ArtifactReviewSubmission};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewArtifactInput {
    pub path: PathBuf,
    pub format: ArtifactFormat,
}

#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum ReviewArtifactOutput {
    Approved,
    Feedback { feedback: String },
    Cancelled,
    Declined,
}

pub async fn execute_review_artifact(
    root_dir: &Path,
    input: ReviewArtifactInput,
    responses: InputResponses,
    context: &RequestContext<RoleServer>,
) -> Result<CallToolResponse, McpError> {
    if let Some(responses) = responses.0 {
        let result: ElicitResult = parse_response(&responses, REVIEW)?;
        return Json(parse_review_result(result)?).into_call_tool_result();
    }

    let path = resolve_path(root_dir, input.path);
    let markdown = match read_file(&path).await {
        Ok(markdown) => markdown,
        Err(error) => return Ok(CallToolResult::error(vec![ContentBlock::text(error)]).into()),
    };

    let params = build_elicitation_form(&path, &markdown, input.format)?;
    let requests = InputRequests::from([(REVIEW.to_string(), InputRequest::Elicitation(ElicitRequest::new(params)))]);

    if !input_requests_supported(context.client_capabilities().as_ref(), &requests) {
        return Ok(CallToolResult::error(vec![ContentBlock::text(ELICITATION_UNSUPPORTED)]).into());
    }

    Ok(InputRequiredResult::from_input_requests(requests).into())
}

const REVIEW: &str = "review";

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
    path: &Path,
    markdown: &str,
    format: ArtifactFormat,
) -> Result<ElicitRequestParams, McpError> {
    let meta =
        ArtifactReviewElicitationMeta::new(path, markdown, format).to_json().map(RequestMetaObject::from).map_err(
            |error| McpError::internal_error(format!("failed to serialize artifact review metadata: {error}"), None),
        )?;

    let requested_schema = ElicitationSchema::builder()
        .required_enum_schema(
            "decision",
            EnumSchema::builder(ArtifactReviewSubmission::DECISIONS.map(String::from).to_vec()).build(),
        )
        .optional_string("feedback")
        .build()
        .map_err(|error| McpError::internal_error(format!("failed to build schema: {error}"), None))?;

    Ok(ElicitRequestParams::FormElicitationParams {
        meta: Some(meta),
        message: format!("Review {} and approve or submit feedback.", path.display()),
        requested_schema,
    })
}

fn parse_review_result(result: ElicitResult) -> Result<ReviewArtifactOutput, McpError> {
    match result.action {
        ElicitationAction::Cancel => Ok(ReviewArtifactOutput::Cancelled),
        ElicitationAction::Decline => Ok(ReviewArtifactOutput::Declined),
        ElicitationAction::Accept => {
            let content = result
                .content
                .ok_or_else(|| McpError::invalid_params("accepted review response is missing content", None))?;
            let submission: ArtifactReviewSubmission = serde_json::from_value(content).map_err(|error| {
                McpError::invalid_params(format!("invalid accepted review response: {error}"), None)
            })?;

            Ok(match submission {
                ArtifactReviewSubmission::Approved => ReviewArtifactOutput::Approved,
                ArtifactReviewSubmission::Feedback { feedback } => ReviewArtifactOutput::Feedback { feedback },
            })
        }
        _ => Err(McpError::invalid_params("unknown elicitation action", None)),
    }
}
