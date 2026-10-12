use rmcp::ErrorData;
use rmcp::model::{CallToolResult, CancelTaskParams, GetTaskParams, GetTaskResult, Task, UpdateTaskParams};
use rmcp::task_manager::{TaskContext, TaskExit, TaskManager, TaskOptions};
use std::future::Future;

pub(crate) struct BackgroundTasks {
    manager: TaskManager,
}

impl BackgroundTasks {
    pub(crate) fn new() -> Self {
        Self { manager: TaskManager::new() }
    }

    pub(crate) fn spawn(
        &self,
        status: impl Into<String>,
        work: impl Future<Output = Result<CallToolResult, TaskExit>> + Send + 'static,
    ) -> Task {
        let options = TaskOptions::new().with_ttl_ms(BACKGROUND_TASK_TTL_MS).with_status_message(status);
        self.manager.spawn(options, move |context: TaskContext| {
            Box::pin(async move {
                tokio::select! {
                    () = context.cancelled() => Err(TaskExit::Cancelled),
                    result = work => result,
                }
            })
        })
    }

    pub(crate) fn get(&self, request: &GetTaskParams) -> Result<GetTaskResult, ErrorData> {
        self.manager.get_task(&request.task_id).map(GetTaskResult::new)
    }

    pub(crate) fn update(&self, request: UpdateTaskParams) -> Result<(), ErrorData> {
        self.manager.update_task(&request.task_id, request.input_responses)
    }

    pub(crate) fn cancel(&self, request: &CancelTaskParams) -> Result<(), ErrorData> {
        self.manager.cancel_task(&request.task_id)
    }
}

impl Drop for BackgroundTasks {
    fn drop(&mut self) {
        self.manager.shutdown();
    }
}

macro_rules! delegate_background_tasks {
    ($tasks:ident) => {
        fn get_task(
            &self,
            request: rmcp::model::GetTaskParams,
            _context: rmcp::service::RequestContext<rmcp::RoleServer>,
        ) -> impl std::future::Future<Output = Result<rmcp::model::GetTaskResult, rmcp::ErrorData>> + Send + '_ {
            std::future::ready(self.$tasks.get(&request))
        }

        fn update_task(
            &self,
            request: rmcp::model::UpdateTaskParams,
            _context: rmcp::service::RequestContext<rmcp::RoleServer>,
        ) -> impl std::future::Future<Output = Result<(), rmcp::ErrorData>> + Send + '_ {
            std::future::ready(self.$tasks.update(request))
        }

        fn cancel_task(
            &self,
            request: rmcp::model::CancelTaskParams,
            _context: rmcp::service::RequestContext<rmcp::RoleServer>,
        ) -> impl std::future::Future<Output = Result<(), rmcp::ErrorData>> + Send + '_ {
            std::future::ready(self.$tasks.cancel(&request))
        }
    };
}

pub(crate) use delegate_background_tasks;

const BACKGROUND_TASK_TTL_MS: u64 = 3_600_000;
