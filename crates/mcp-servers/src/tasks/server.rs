use clap::Parser;
use rmcp::{
    ServerHandler,
    handler::server::{
        router::tool::ToolRouter,
        wrapper::{Json, Parameters},
    },
    model::{Implementation, ServerCapabilities, ServerConfig},
    tool, tool_handler, tool_router,
};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tempfile::TempDir;

use crate::error::ServerInitError;
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
pub struct TasksMcpArgs {
    /// Base directory for persistent task storage. If omitted, tasks are
    /// session-scoped and stored in a temporary directory that is cleaned up
    /// when the server is dropped.
    #[arg(long = "dir")]
    pub dir: Option<PathBuf>,
}

impl TasksMcpArgs {
    pub fn from_args(args: Vec<String>) -> Result<Self, ServerInitError> {
        let mut full_args = vec!["tasks-mcp".to_string()];
        full_args.extend(args);

        Self::try_parse_from(full_args).map_err(ServerInitError::InvalidArgs)
    }
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

    /// Create a new `TasksMcp` server from parsed CLI arguments.
    ///
    /// If `--dir` is provided, tasks persist at that path. Otherwise, tasks are
    /// session-scoped in a temporary directory.
    pub fn from_args(args: Vec<String>) -> Result<Self, ServerInitError> {
        let parsed_args = TasksMcpArgs::from_args(args)?;
        Ok(match parsed_args.dir {
            Some(dir) => Self::new_persistent(dir),
            None => Self::new(),
        })
    }

    pub fn from_args_with_base_dir(args: Vec<String>, base_dir: &Path) -> Result<Self, ServerInitError> {
        let parsed_args = TasksMcpArgs::from_args(args)?;
        Ok(match parsed_args.dir {
            Some(dir) => Self::new_persistent(resolve_path(base_dir, dir)),
            None => Self::new(),
        })
    }

    #[cfg(test)]
    #[allow(clippy::used_underscore_binding)]
    fn is_session_scoped(&self) -> bool {
        self.storage._temp_dir.is_some()
    }

    #[cfg(test)]
    #[allow(clippy::used_underscore_binding)]
    fn temp_path(&self) -> Option<PathBuf> {
        self.storage._temp_dir.as_ref().map(|d| d.path().to_path_buf())
    }
}

#[allow(clippy::unused_async_trait_impl)]
#[tool_handler(router = self.tool_router)]
impl ServerHandler for TasksMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("tasks-mcp", "0.1.0"))
            .with_instructions(include_str!("./instructions.md"))
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

    #[test]
    fn test_new_is_session_scoped() {
        let server = TasksMcp::new();
        assert!(server.is_session_scoped());
        let temp_path = server.temp_path().unwrap();
        assert!(temp_path.exists());

        drop(server);
        assert!(!temp_path.exists(), "temp dir should be cleaned up on drop");
    }

    #[test]
    fn test_new_persistent_uses_provided_dir() {
        let temp = TempDir::new().unwrap();
        let server = TasksMcp::new_persistent(temp.path().to_path_buf());
        assert!(!server.is_session_scoped());
    }

    #[test]
    fn test_from_args_no_dir_is_session_scoped() {
        let server = TasksMcp::from_args(vec![]).unwrap();
        assert!(server.is_session_scoped());
    }

    #[test]
    fn test_from_args_with_base_dir_resolves_relative_dir() {
        let server =
            TasksMcp::from_args_with_base_dir(vec!["--dir".into(), "tasks".into()], Path::new("/workspace")).unwrap();

        assert!(!server.is_session_scoped());
    }

    #[test]
    fn test_from_args_with_base_dir_no_dir_stays_session_scoped() {
        let server = TasksMcp::from_args_with_base_dir(vec![], Path::new("/workspace")).unwrap();
        assert!(server.is_session_scoped());
    }
}
