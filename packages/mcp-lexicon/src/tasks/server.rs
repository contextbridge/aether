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
use std::path::PathBuf;
use tokio::sync::{Mutex, RwLock};

use crate::tasks::{
    TaskCreateInput, TaskCreateOutput, TaskGetInput, TaskGetOutput, TaskListInput, TaskListOutput,
    TaskStore, TaskUpdateInput, TaskUpdateOutput, execute_task_create, execute_task_get,
    execute_task_list, execute_task_update,
};

/// CLI arguments for TasksMcp server
#[derive(Debug, Clone, Parser)]
pub struct TasksMcpArgs {
    /// Base directory for task storage (tasks stored in `{dir}/.aether-tasks/`)
    #[arg(long = "dir", default_value = ".")]
    pub dir: PathBuf,
}

impl TasksMcpArgs {
    pub fn from_args(args: Vec<String>) -> Result<Self, String> {
        // Prepend a dummy program name since clap expects it
        let mut full_args = vec!["tasks-mcp".to_string()];
        full_args.extend(args);

        Self::try_parse_from(full_args)
            .map_err(|e| format!("Failed to parse TasksMcp arguments: {e}"))
    }
}

/// MCP server for task management in deep research agent workflows.
///
/// Provides tools for creating, listing, and updating tasks organized into
/// hierarchical trees with dependency tracking.
#[derive(Debug)]
pub struct TasksMcp {
    task_store: Mutex<TaskStore>,
    tool_router: ToolRouter<Self>,
    /// Workspace roots (from MCP protocol or CLI args)
    roots: RwLock<Vec<PathBuf>>,
}

impl TasksMcp {
    /// Create a new TasksMcp server with task storage in the given directory.
    ///
    /// Tasks will be stored in `{base_dir}/.aether-tasks/`.
    pub fn new(base_dir: PathBuf) -> Self {
        Self {
            task_store: Mutex::new(TaskStore::new(base_dir.join(".aether-tasks"))),
            tool_router: Self::tool_router(),
            roots: RwLock::new(vec![base_dir]),
        }
    }

    /// Create a new TasksMcp server from parsed CLI arguments.
    ///
    /// If no `--dir` argument is provided, uses the current directory.
    pub fn from_args(args: Vec<String>) -> Result<Self, String> {
        let parsed_args = TasksMcpArgs::from_args(args)?;
        Ok(Self::new(parsed_args.dir))
    }

    /// Set workspace roots.
    ///
    /// Can be used to set roots from MCP protocol or to override CLI arguments.
    pub fn with_roots(mut self, roots: Vec<PathBuf>) -> Self {
        self.roots = RwLock::new(roots);
        self
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for TasksMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            server_info: Implementation {
                name: "tasks-mcp".to_string(),
                version: "0.1.0".to_string(),
                title: None,
                icons: None,
                website_url: None,
            },
            instructions: Some(include_str!("./instructions.md").to_string()),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

#[tool_router]
impl TasksMcp {
    #[doc = include_str!("./tools/create/description.md")]
    #[tool]
    pub async fn task_create(
        &self,
        request: Parameters<TaskCreateInput>,
    ) -> Result<Json<TaskCreateOutput>, String> {
        let Parameters(input) = request;
        let mut store = self.task_store.lock().await;
        store.init().map_err(|e| e.to_string())?;
        execute_task_create(input, &mut store)
            .map(Json)
            .map_err(|e| e.to_string())
    }

    #[doc = include_str!("./tools/update/description.md")]
    #[tool]
    pub async fn task_update(
        &self,
        request: Parameters<TaskUpdateInput>,
    ) -> Result<Json<TaskUpdateOutput>, String> {
        let Parameters(input) = request;
        let mut store = self.task_store.lock().await;
        store.init().map_err(|e| e.to_string())?;
        execute_task_update(input, &mut store)
            .map(Json)
            .map_err(|e| e.to_string())
    }

    #[doc = include_str!("./tools/list/description.md")]
    #[tool]
    pub async fn task_list(
        &self,
        request: Parameters<TaskListInput>,
    ) -> Result<Json<TaskListOutput>, String> {
        let Parameters(input) = request;
        let mut store = self.task_store.lock().await;
        store.init().map_err(|e| e.to_string())?;
        Ok(Json(execute_task_list(input, &store)))
    }

    #[doc = include_str!("./tools/get/description.md")]
    #[tool]
    pub async fn task_get(
        &self,
        request: Parameters<TaskGetInput>,
    ) -> Result<Json<TaskGetOutput>, String> {
        let Parameters(input) = request;
        let mut store = self.task_store.lock().await;
        store.init().map_err(|e| e.to_string())?;
        execute_task_get(input, &store)
            .map(Json)
            .map_err(|e| e.to_string())
    }
}
