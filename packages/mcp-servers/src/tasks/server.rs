use clap::Parser;
use rmcp::{
    ServerHandler,
    handler::server::{
        router::tool::ToolRouter,
        wrapper::{Json, Parameters},
    },
    model::{Implementation, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
};
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use tokio::sync::{Mutex, RwLock};

use crate::{
    tasks::{
        TaskCreateInput, TaskCreateOutput, TaskGetInput, TaskGetOutput, TaskListInput, TaskListOutput, TaskStore,
        TaskUpdateInput, TaskUpdateOutput, execute_task_create, execute_task_get, execute_task_list,
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
    pub fn from_args(args: Vec<String>) -> Result<Self, String> {
        // Prepend a dummy program name since clap expects it
        let mut full_args = vec!["tasks-mcp".to_string()];
        full_args.extend(args);

        Self::try_parse_from(full_args).map_err(|e| format!("Failed to parse TasksMcp arguments: {e}"))
    }
}

#[doc = include_str!("../docs/tasks_mcp.md")]
#[derive(Debug)]
pub struct TasksMcp {
    task_store: Mutex<TaskStore>,
    tool_router: ToolRouter<Self>,
    /// Workspace roots (from MCP protocol or CLI args)
    roots: RwLock<Vec<PathBuf>>,
    /// Holds the temp directory alive for session-scoped storage.
    /// Dropped (and cleaned up) when the server is dropped.
    _temp_dir: Option<TempDir>,
}

impl Default for TasksMcp {
    fn default() -> Self {
        Self::new()
    }
}

impl TasksMcp {
    /// Create a new session-scoped `TasksMcp` server.
    ///
    /// Tasks are stored in a temporary directory and automatically cleaned up
    /// when this server is dropped. Use [`Self::new_persistent`] for
    /// cross-session storage.
    pub fn new() -> Self {
        let temp_dir = TempDir::with_prefix("aether-tasks-").expect("failed to create temp dir for task storage");
        let task_path = temp_dir.path().to_path_buf();
        Self {
            task_store: Mutex::new(TaskStore::new(task_path)),
            tool_router: Self::tool_router(),
            roots: RwLock::new(vec![]),
            _temp_dir: Some(temp_dir),
        }
    }

    /// Create a new `TasksMcp` server with persistent task storage.
    ///
    /// Tasks will be stored in `{base_dir}/.aether-tasks/` and persist across
    /// sessions.
    pub fn new_persistent(base_dir: PathBuf) -> Self {
        Self {
            task_store: Mutex::new(TaskStore::new(base_dir.join(".aether-tasks"))),
            tool_router: Self::tool_router(),
            roots: RwLock::new(vec![base_dir]),
            _temp_dir: None,
        }
    }

    /// Create a new `TasksMcp` server from parsed CLI arguments.
    ///
    /// If `--dir` is provided, tasks persist at that path. Otherwise, tasks are
    /// session-scoped in a temporary directory.
    pub fn from_args(args: Vec<String>) -> Result<Self, String> {
        let parsed_args = TasksMcpArgs::from_args(args)?;
        Ok(match parsed_args.dir {
            Some(dir) => Self::new_persistent(dir),
            None => Self::new(),
        })
    }

    pub fn from_args_with_workspace_root(args: Vec<String>, workspace_root: &Path) -> Result<Self, String> {
        let parsed_args = TasksMcpArgs::from_args(args)?;
        Ok(match parsed_args.dir {
            Some(dir) => Self::new_persistent(resolve_path(workspace_root, dir)),
            None => Self::new().with_roots(vec![workspace_root.to_path_buf()]),
        })
    }

    /// Set workspace roots.
    ///
    /// Can be used to set roots from MCP protocol or to override CLI arguments.
    pub fn with_roots(mut self, roots: Vec<PathBuf>) -> Self {
        self.roots = RwLock::new(roots);
        self
    }

    #[cfg(test)]
    #[allow(clippy::used_underscore_binding)]
    fn is_session_scoped(&self) -> bool {
        self._temp_dir.is_some()
    }

    #[cfg(test)]
    #[allow(clippy::used_underscore_binding)]
    fn temp_path(&self) -> Option<PathBuf> {
        self._temp_dir.as_ref().map(|d| d.path().to_path_buf())
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for TasksMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("tasks-mcp", "0.1.0"))
            .with_instructions(include_str!("./instructions.md"))
    }
}

#[tool_router]
impl TasksMcp {
    #[doc = include_str!("./tools/create/description.md")]
    #[tool]
    pub async fn task_create(&self, request: Parameters<TaskCreateInput>) -> Result<Json<TaskCreateOutput>, String> {
        let Parameters(input) = request;
        let mut store = self.task_store.lock().await;
        store.init().map_err(|e| e.to_string())?;
        execute_task_create(&input, &mut store).map(Json).map_err(|e| e.to_string())
    }

    #[doc = include_str!("./tools/update/description.md")]
    #[tool]
    pub async fn task_update(&self, request: Parameters<TaskUpdateInput>) -> Result<Json<TaskUpdateOutput>, String> {
        let Parameters(input) = request;
        let mut store = self.task_store.lock().await;
        store.init().map_err(|e| e.to_string())?;
        execute_task_update(input, &mut store).map(Json).map_err(|e| e.to_string())
    }

    #[doc = include_str!("./tools/list/description.md")]
    #[tool]
    pub async fn task_list(&self, request: Parameters<TaskListInput>) -> Result<Json<TaskListOutput>, String> {
        let Parameters(input) = request;
        let mut store = self.task_store.lock().await;
        store.init().map_err(|e| e.to_string())?;
        Ok(Json(execute_task_list(&input, &store)))
    }

    #[doc = include_str!("./tools/get/description.md")]
    #[tool]
    pub async fn task_get(&self, request: Parameters<TaskGetInput>) -> Result<Json<TaskGetOutput>, String> {
        let Parameters(input) = request;
        let mut store = self.task_store.lock().await;
        store.init().map_err(|e| e.to_string())?;
        execute_task_get(input, &store).map(Json).map_err(|e| e.to_string())
    }
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
    fn test_from_args_with_workspace_root_resolves_relative_dir() {
        let server =
            TasksMcp::from_args_with_workspace_root(vec!["--dir".into(), "tasks".into()], Path::new("/workspace"))
                .unwrap();

        assert!(!server.is_session_scoped());
        assert_eq!(server.roots.blocking_read().as_slice(), &[PathBuf::from("/workspace/tasks")]);
    }

    #[test]
    fn test_from_args_with_workspace_root_no_dir_stays_session_scoped() {
        let server = TasksMcp::from_args_with_workspace_root(vec![], Path::new("/workspace")).unwrap();
        assert!(server.is_session_scoped());
        assert_eq!(server.roots.blocking_read().as_slice(), &[PathBuf::from("/workspace")]);
    }
}
