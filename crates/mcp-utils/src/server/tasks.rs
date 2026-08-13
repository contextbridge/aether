use rmcp::model::ClientCapabilities;
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer};

/// Bounds both task execution and terminal-result retention because rmcp uses
/// one TTL for both concerns.
pub const BACKGROUND_TASK_TTL_MS: u64 = 3_600_000;

pub fn require_tasks_capability(context: &RequestContext<RoleServer>) -> Result<(), ErrorData> {
    if context.client_capabilities().is_some_and(|capabilities| capabilities.supports_tasks()) {
        Ok(())
    } else {
        Err(ErrorData::missing_required_client_capability(ClientCapabilities::builder().enable_tasks().build()))
    }
}
