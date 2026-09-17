use rmcp::model::ClientCapabilities;
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer};

/// Bounds both task execution and terminal-result retention because rmcp uses
/// one TTL for both concerns.
pub const BACKGROUND_TASK_TTL_MS: u64 = 3_600_000;

/// Identifies the task backing a foreground continuation for cancellation.
pub const TASK_CONTINUATION_KEY: &str = "aether-agent.io/task-continuation";

/// Adapt existing task storage to foreground MRTR without restarting execution.
pub fn task_to_mrtr(
    manager: &rmcp::task_manager::TaskManager,
    task_id: &str,
    responses: Option<rmcp::model::InputResponses>,
) -> Result<rmcp::model::CallToolResponse, ErrorData> {
    use rmcp::model::{CallToolResult, InputRequiredResult, MetaObject, TaskPayload};
    if let Some(responses) = responses {
        manager.update_task(task_id, responses)?;
    }
    let task = manager.get_task(task_id)?;
    let inputs = match task.payload {
        TaskPayload::Working => None,
        TaskPayload::InputRequired { input_requests } => Some(input_requests),
        TaskPayload::Completed { result } => {
            return serde_json::from_value::<CallToolResult>(serde_json::Value::Object(result))
                .map(Into::into)
                .map_err(|_| ErrorData::internal_error("invalid task result", None));
        }
        TaskPayload::Failed { .. } => return Err(ErrorData::internal_error("execution task failed or expired", None)),
        TaskPayload::Cancelled => return Err(ErrorData::internal_error("execution task cancelled", None)),
        _ => return Err(ErrorData::internal_error("unsupported execution task state", None)),
    };
    let mut meta = MetaObject::default();
    meta.insert(TASK_CONTINUATION_KEY.into(), task_id.into());
    Ok(InputRequiredResult::new(inputs, Some(task_id.to_string())).with_meta(meta).into())
}

pub fn require_tasks_capability(context: &RequestContext<RoleServer>) -> Result<(), ErrorData> {
    if context.client_capabilities().is_some_and(|capabilities| capabilities.supports_tasks()) {
        Ok(())
    } else {
        Err(ErrorData::missing_required_client_capability(ClientCapabilities::builder().enable_tasks().build()))
    }
}
