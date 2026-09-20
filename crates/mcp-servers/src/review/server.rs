use super::tools::{ReviewArtifactInput, ReviewArtifactOutput, ReviewArtifactTool};
use crate::error::ServerInitError;
use crate::workspace_paths::{current_dir, resolve_path};
use clap::Parser;
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    handler::server::{
        router::tool::ToolRouter,
        tool::{InputResponses, RequestState, schema_for_output},
        wrapper::Parameters,
    },
    model::{CallToolResponse, Implementation, ServerCapabilities, ServerConfig},
    service::RequestContext,
    tool, tool_handler, tool_router,
};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone, Parser)]
#[command(name = "review-mcp")]
pub struct ReviewMcpArgs {
    /// Workspace root used to resolve relative artifact paths.
    #[arg(long)]
    pub root_dir: Option<PathBuf>,
}

impl ReviewMcpArgs {
    pub fn from_args(args: Vec<String>) -> Result<Self, ServerInitError> {
        let mut full_args = vec!["review-mcp".to_string()];
        full_args.extend(args);
        Self::try_parse_from(full_args).map_err(ServerInitError::InvalidArgs)
    }
}

#[doc = include_str!("../docs/review_mcp.md")]
#[derive(Clone)]
pub struct ReviewMcp {
    tool_router: ToolRouter<Self>,
    review_artifact: Arc<ReviewArtifactTool>,
}

#[tool_router]
impl ReviewMcp {
    pub fn new() -> Self {
        Self::at_root(current_dir())
    }

    pub fn from_args(args: Vec<String>) -> Result<Self, ServerInitError> {
        let parsed = ReviewMcpArgs::from_args(args)?;
        Ok(Self::at_root(parsed.root_dir.unwrap_or_else(current_dir)))
    }

    pub fn from_args_with_base_dir(args: Vec<String>, base_dir: &std::path::Path) -> Result<Self, ServerInitError> {
        let parsed = ReviewMcpArgs::from_args(args)?;
        let root_dir = parsed.root_dir.map_or_else(|| base_dir.to_path_buf(), |path| resolve_path(base_dir, path));
        Ok(Self::at_root(root_dir))
    }

    #[doc = include_str!("tools/review_artifact/description.md")]
    #[tool(
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = false, open_world_hint = false),
        output_schema = schema_for_output::<ReviewArtifactOutput>()
    )]
    pub async fn review_artifact(
        &self,
        request: Parameters<ReviewArtifactInput>,
        responses: InputResponses,
        request_state: RequestState,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        self.review_artifact.execute(request.0, responses, request_state, &context).await
    }
}

#[allow(clippy::unused_async_trait_impl)]
#[tool_handler(router = self.tool_router)]
impl ServerHandler for ReviewMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("review-mcp", "0.1.0"))
            .with_instructions(include_str!("./instructions.md"))
    }
}

impl ReviewMcp {
    fn at_root(root_dir: PathBuf) -> Self {
        Self { tool_router: Self::tool_router(), review_artifact: Arc::new(ReviewArtifactTool::new(root_dir)) }
    }
}

impl Default for ReviewMcp {
    fn default() -> Self {
        Self::new()
    }
}
