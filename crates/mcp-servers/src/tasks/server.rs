use crate::server_config;
use clap::Parser;
use rmcp::{
    ServerHandler,
    handler::server::{
        router::tool::ToolRouter,
        wrapper::{Json, Parameters},
    },
    model::{ServerCapabilities, ServerConfig},
    tool, tool_handler, tool_router,
};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tempfile::TempDir;

use crate::{
    tasks::{
        TaskCreateInput, TaskCreateOutput, TaskGetInput, TaskGetOutput, TaskListInput, TaskListOutput, TaskStore,
        TaskStoreError, TaskUpdateInput, TaskUpdateOutput, execute_task_create, execute_task_get, execute_task_list,
        execute_task_update,
    },
    workspace_paths::resolve_path,
};

/// CLI arguments for `TasksMcp` server
#[derive(Debug, Clone, Parser)]
#[command(name = "tasks", no_binary_name = true)]
pub struct TasksMcpArgs {
    /// Base directory for persistent task storage. If omitted, tasks are
    /// session-scoped and stored in a temporary directory that is cleaned up
    /// when the server is dropped.
    #[arg(long = "dir")]
    pub dir: Option<PathBuf>,
}

#[doc = include_str!("../docs/tasks_mcp.md")]
#[derive(Debug)]
pub struct TasksMcp {
    storage: Arc<TaskStorage>,
    tool_router: ToolRouter<Self>,
}

impl Default for TasksMcp {
    fn default() -> Self {
        Self::new()
    }
}

impl TasksMcp {
    /// Create a new session-scoped `TasksMcp` server.
    ///
    /// Tasks are stored in a temporary directory and cleaned up after the server
    /// is dropped and outstanding operations finish. Use [`Self::new_persistent`]
    /// for cross-session storage.
    pub fn new() -> Self {
        let temp_dir = TempDir::with_prefix("aether-tasks-").expect("failed to create temp dir for task storage");
        let task_path = temp_dir.path().to_path_buf();
        Self {
            storage: Arc::new(TaskStorage { store: Mutex::new(TaskStore::new(task_path)), _temp_dir: Some(temp_dir) }),
            tool_router: Self::tool_router(),
        }
    }

    /// Create a new `TasksMcp` server with persistent task storage.
    ///
    /// Tasks will be stored in `{base_dir}/.aether-tasks/` and persist across
    /// sessions.
    pub fn new_persistent(base_dir: impl Into<PathBuf>) -> Self {
        let base_dir = base_dir.into();
        Self {
            storage: Arc::new(TaskStorage {
                store: Mutex::new(TaskStore::new(base_dir.join(".aether-tasks"))),
                _temp_dir: None,
            }),
            tool_router: Self::tool_router(),
        }
    }

    pub fn from_args(args: TasksMcpArgs, base_dir: &Path) -> Self {
        match args.dir {
            Some(dir) => Self::new_persistent(resolve_path(base_dir, dir)),
            None => Self::new(),
        }
    }
}

#[allow(clippy::unused_async_trait_impl)]
#[tool_handler(router = self.tool_router)]
impl ServerHandler for TasksMcp {
    fn get_info(&self) -> ServerConfig {
        server_config(
            "tasks-mcp",
            ServerCapabilities::builder().enable_tools().build(),
            include_str!("./instructions.md"),
        )
    }
}

#[tool_router]
impl TasksMcp {
    #[doc = include_str!("./tools/create/description.md")]
    #[tool(annotations(
        read_only_hint = false,
        destructive_hint = false,
        idempotent_hint = false,
        open_world_hint = false
    ))]
    pub async fn task_create(
        &self,
        request: Parameters<TaskCreateInput>,
    ) -> Result<Json<TaskCreateOutput>, TaskStoreError> {
        let Parameters(input) = request;
        self.with_store(move |store| execute_task_create(&input, store)).await.map(Json)
    }

    #[doc = include_str!("./tools/update/description.md")]
    #[tool(annotations(
        read_only_hint = false,
        destructive_hint = true,
        idempotent_hint = false,
        open_world_hint = false
    ))]
    pub async fn task_update(
        &self,
        request: Parameters<TaskUpdateInput>,
    ) -> Result<Json<TaskUpdateOutput>, TaskStoreError> {
        let Parameters(input) = request;
        self.with_store(move |store| execute_task_update(input, store)).await.map(Json)
    }

    #[doc = include_str!("./tools/list/description.md")]
    #[tool(annotations(read_only_hint = true, open_world_hint = false))]
    pub async fn task_list(&self, request: Parameters<TaskListInput>) -> Result<Json<TaskListOutput>, TaskStoreError> {
        let Parameters(input) = request;
        self.with_store(move |store| Ok(execute_task_list(&input, store))).await.map(Json)
    }

    #[doc = include_str!("./tools/get/description.md")]
    #[tool(annotations(read_only_hint = true, open_world_hint = false))]
    pub async fn task_get(&self, request: Parameters<TaskGetInput>) -> Result<Json<TaskGetOutput>, TaskStoreError> {
        let Parameters(input) = request;
        self.with_store(move |store| execute_task_get(input, store)).await.map(Json)
    }
}

impl TasksMcp {
    async fn with_store<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut TaskStore) -> Result<T, TaskStoreError> + Send + 'static,
    ) -> Result<T, TaskStoreError> {
        let storage = Arc::clone(&self.storage);
        tokio::task::spawn_blocking(move || {
            let mut store = storage.store.lock().expect("task store lock poisoned");
            store.init()?;
            operation(&mut store)
        })
        .await?
    }
}

#[derive(Debug)]
struct TaskStorage {
    store: Mutex<TaskStore>,
    /// Keep session storage alive until outstanding blocking operations finish.
    _temp_dir: Option<TempDir>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::TaskId;
    use std::fs;

    #[tokio::test]
    async fn session_storage_is_removed_on_drop() {
        let server = TasksMcp::new();
        let id = create_task(&server).await;
        let file = session_task_file(&id).expect("session task file should exist in a temp dir");

        drop(server);
        assert!(!file.exists(), "temp dir should be cleaned up on drop");
    }

    #[tokio::test]
    async fn from_args_without_dir_leaves_base_dir_untouched() {
        let base = TempDir::new().unwrap();
        let server = TasksMcp::from_args(TasksMcpArgs::try_parse_from(Vec::<String>::new()).unwrap(), base.path());

        create_task(&server).await;
        assert_eq!(fs::read_dir(base.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn from_args_with_dir_persists_across_servers() {
        let base = TempDir::new().unwrap();
        let args = || TasksMcpArgs::try_parse_from(["--dir", "tasks"]).unwrap();
        let id = create_task(&TasksMcp::from_args(args(), base.path())).await;

        assert!(base.path().join("tasks/.aether-tasks/active").join(id.filename()).exists());
        let reopened = TasksMcp::from_args(args(), base.path());
        let Json(listed) = reopened.task_list(Parameters(TaskListInput::default())).await.unwrap();
        assert_eq!(listed.tasks.len(), 1);
    }

    async fn create_task(server: &TasksMcp) -> TaskId {
        let input =
            TaskCreateInput { title: "Task".into(), description: None, parent_id: None, assignee: None, deps: None };
        let Json(output) = server.task_create(Parameters(input)).await.unwrap();
        TaskId::from(output.task.id)
    }

    fn session_task_file(id: &TaskId) -> Option<PathBuf> {
        fs::read_dir(std::env::temp_dir())
            .ok()?
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("aether-tasks-"))
            .map(|entry| entry.path().join("active").join(id.filename()))
            .find(|path| path.exists())
    }
}
