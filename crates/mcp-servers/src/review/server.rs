use super::tools::{ReviewArtifactInput, ReviewArtifactOutput, ReviewArtifactTool};
use crate::server_config;
use crate::workspace_paths::resolve_root;
use clap::Parser;
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    handler::server::{
        router::tool::ToolRouter,
        tool::{InputResponses, RequestState, schema_for_output},
        wrapper::Parameters,
    },
    model::{CallToolResponse, ServerCapabilities, ServerConfig},
    service::RequestContext,
    tool, tool_handler, tool_router,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone, Parser)]
#[command(name = "review", no_binary_name = true)]
pub struct ReviewMcpArgs {
    /// Workspace root used to resolve relative artifact paths.
    #[arg(long)]
    pub root_dir: Option<PathBuf>,
}

#[doc = include_str!("../docs/review_mcp.md")]
#[derive(Clone)]
pub struct ReviewMcp {
    tool_router: ToolRouter<Self>,
    review_artifact: Arc<ReviewArtifactTool>,
}

impl ReviewMcp {
    pub fn new(root_dir: PathBuf) -> Self {
        Self { tool_router: Self::tool_router(), review_artifact: Arc::new(ReviewArtifactTool::new(root_dir)) }
    }

    pub fn from_args(args: ReviewMcpArgs, base_dir: &Path) -> Self {
        Self::new(resolve_root(base_dir, args.root_dir))
    }
}

#[tool_router]
impl ReviewMcp {
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
        server_config(
            "review-mcp",
            ServerCapabilities::builder().enable_tools().build(),
            include_str!("./instructions.md"),
        )
    }
}
