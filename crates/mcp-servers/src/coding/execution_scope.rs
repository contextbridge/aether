use super::tools::bash::BashEnvironment;
use mcp_utils::request_context::GatewayRequestContext;
use rmcp::{ErrorData, task_manager::TaskContext};

/// Supplies a task-bound environment for remote Bash composition.
pub trait BashTaskScope: Send + Sync {
    fn environment(&self, task: TaskContext, context: GatewayRequestContext) -> Result<BashEnvironment, ErrorData>;
}
