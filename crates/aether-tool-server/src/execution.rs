use mcp_servers::coding::{execution_scope::BashTaskScope, tools::bash::BashEnvironment};
use mcp_utils::{
    client::{InputResponder, cancel_result},
    request_context::{AETHER_MCP_REQUEST_CONTEXT, AgentIdentity, GatewayRequestContext},
};
use rmcp::{
    ErrorData,
    model::{ElicitRequest, ElicitRequestParams, ElicitResult, InputRequest},
    task_manager::TaskContext,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::time::Instant;
use uuid::Uuid;

#[derive(Default)]
pub(crate) struct ExecutionTasks {
    tasks: Mutex<HashMap<String, ExecutionTask>>,
}

impl ExecutionTasks {
    pub fn responder(&self, policy: &GatewayRequestContext) -> Result<Arc<dyn InputResponder>, ErrorData> {
        let id = policy.execution_task.as_ref().ok_or_else(invalid_task)?;
        let tasks = self.tasks.lock().expect("execution tasks poisoned");
        let task = tasks.get(id).ok_or_else(invalid_task)?;
        if task.owner != policy.identity || task.expires <= Instant::now() {
            return Err(invalid_task());
        }
        Ok(Arc::new(TaskResponder(task.context.clone())))
    }
}

impl BashTaskScope for ExecutionTasks {
    fn environment(&self, task: TaskContext, mut context: GatewayRequestContext) -> Result<BashEnvironment, ErrorData> {
        let id = Uuid::new_v4().to_string();
        let mut tasks = self.tasks.lock().expect("execution tasks poisoned");
        tasks.retain(|_, task| task.expires > Instant::now());
        tasks.insert(
            id.clone(),
            ExecutionTask {
                owner: context.identity,
                context: task,
                expires: Instant::now() + Duration::from_secs(3600),
            },
        );
        context.execution_task = Some(id);
        let value = serde_json::to_string(&context).map_err(|_| invalid_task())?;
        Ok(BashEnvironment::new().with_var(AETHER_MCP_REQUEST_CONTEXT, value))
    }
}

struct ExecutionTask {
    owner: AgentIdentity,
    context: TaskContext,
    expires: Instant,
}

struct TaskResponder(TaskContext);

impl InputResponder for TaskResponder {
    fn elicit(&self, request: ElicitRequestParams) -> futures::future::BoxFuture<'_, ElicitResult> {
        Box::pin(async move {
            self.0
                .request_input(Uuid::new_v4().to_string(), InputRequest::Elicitation(ElicitRequest::new(request)))
                .await
                .ok()
                .and_then(|value| serde_json::from_value(value).ok())
                .unwrap_or_else(cancel_result)
        })
    }
}

fn invalid_task() -> ErrorData {
    ErrorData::invalid_params("missing, expired, or unauthorized outer execution task", None)
}
