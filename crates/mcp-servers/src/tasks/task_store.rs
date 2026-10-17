use super::types::{Task, TaskId, TaskStatus, TaskUpdate};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use thiserror::Error;
use utils::fs::append_line;

/// Errors that can occur during task store operations
#[derive(Debug, Error)]
pub enum TaskStoreError {
    #[error("Task not found: {id}")]
    NotFound { id: String },

    #[error("Task already exists: {id}")]
    AlreadyExists { id: String },

    #[error("Parent task not found: {id}")]
    ParentNotFound { id: String },

    #[error("Dependency not found: {id}")]
    DependencyNotFound { id: String },

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Task store worker failed: {0}")]
    Worker(#[from] tokio::task::JoinError),
}

/// Renders the error as the text content of an errored MCP tool result.
impl rmcp::model::IntoContents for TaskStoreError {
    fn into_contents(self) -> Vec<rmcp::model::ContentBlock> {
        vec![rmcp::model::ContentBlock::text(self.to_string())]
    }
}

/// Task store backed by one JSONL file per task tree, held in memory in [`TaskId`] order
#[derive(Debug)]
pub struct TaskStore {
    root: PathBuf,
    tasks: BTreeMap<TaskId, Task>,
    initialized: bool,
}

impl TaskStore {
    /// Create a new task store at the given root directory
    pub fn new(root: PathBuf) -> Self {
        Self { root, tasks: BTreeMap::new(), initialized: false }
    }

    /// Initialize the store by loading all active tasks.
    /// This is idempotent - subsequent calls are no-ops.
    pub fn init(&mut self) -> Result<(), TaskStoreError> {
        if self.initialized {
            return Ok(());
        }

        let active_dir = self.active_dir();
        if !active_dir.exists() {
            fs::create_dir_all(&active_dir)?;
        }

        self.tasks.clear();

        for entry in fs::read_dir(&active_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("jsonl") {
                self.load_tree_file(&path)?;
            }
        }

        self.initialized = true;
        Ok(())
    }

    pub fn create(&mut self, parent: Option<&TaskId>, title: &str, fields: TaskUpdate) -> Result<Task, TaskStoreError> {
        if let Some(parent) = parent.filter(|parent| !self.tasks.contains_key(parent)) {
            return Err(TaskStoreError::ParentNotFound { id: parent.to_string() });
        }
        self.ensure_deps_exist(fields.deps.as_deref())?;

        let mut task = if let Some(parent) = parent {
            let id = TaskId::new_subtask(parent, self.next_subtask_index(&parent.root()));
            Task::new_subtask(id, parent.clone(), title.to_string())
        } else {
            let id = TaskId::new_root(&self.generate_hash());
            if self.tasks.contains_key(&id) {
                return Err(TaskStoreError::AlreadyExists { id: id.to_string() });
            }
            Task::new_root(id, title.to_string())
        };
        fields.apply_to(&mut task);

        append_line(&self.tree_path(&task.id), &serde_json::to_string(&task)?)?;
        self.tasks.insert(task.id.clone(), task.clone());

        Ok(task)
    }

    /// Update a task's fields
    pub fn update(&mut self, id: &TaskId, updates: TaskUpdate) -> Result<Task, TaskStoreError> {
        self.ensure_deps_exist(updates.deps.as_deref())?;

        let task = self.tasks.get_mut(id).ok_or_else(|| TaskStoreError::NotFound { id: id.to_string() })?;
        updates.apply_to(task);
        let task = task.clone();

        self.rewrite_tree_file(&task.id.root())?;

        Ok(task)
    }

    /// Get a task by ID
    pub fn get(&self, id: &TaskId) -> Option<&Task> {
        self.tasks.get(id)
    }

    /// All tasks in [`TaskId`] order
    pub fn tasks(&self) -> impl Iterator<Item = &Task> {
        self.tasks.values()
    }

    /// Whether `task` can start: it is pending and every dependency is completed
    pub fn is_ready(&self, task: &Task) -> bool {
        task.status == TaskStatus::Pending
            && task.deps.iter().all(|dep| self.get(dep).is_some_and(|dep| dep.status == TaskStatus::Completed))
    }

    fn active_dir(&self) -> PathBuf {
        self.root.join("active")
    }

    fn tree_path(&self, id: &TaskId) -> PathBuf {
        self.active_dir().join(id.filename())
    }

    fn tree(&self, root_id: &TaskId) -> impl Iterator<Item = &Task> {
        self.tasks().filter(move |task| task.id.root() == *root_id)
    }

    fn next_subtask_index(&self, root_id: &TaskId) -> usize {
        self.tree(root_id).filter_map(|task| task.id.index()).max().unwrap_or(0) + 1
    }

    fn ensure_deps_exist(&self, deps: Option<&[TaskId]>) -> Result<(), TaskStoreError> {
        match deps.into_iter().flatten().find(|dep| !self.tasks.contains_key(dep)) {
            Some(dep) => Err(TaskStoreError::DependencyNotFound { id: dep.to_string() }),
            None => Ok(()),
        }
    }

    fn generate_hash(&self) -> String {
        use std::time::{SystemTime, UNIX_EPOCH};

        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();

        let pid = u128::from(std::process::id());
        let count = self.tasks.len() as u128;
        let mixed = now.wrapping_add(pid << 16).wrapping_add(count);
        // Intentional truncation: value is masked to 32 bits for ID generation
        #[allow(clippy::cast_possible_truncation)]
        let truncated = mixed as u64;
        format!("{:08x}", truncated & 0xFFFF_FFFF)
    }

    fn load_tree_file(&mut self, path: &Path) -> Result<(), TaskStoreError> {
        for line in BufReader::new(File::open(path)?).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }

            let task: Task = serde_json::from_str(&line)?;
            self.tasks.insert(task.id.clone(), task);
        }

        Ok(())
    }

    fn rewrite_tree_file(&self, root_id: &TaskId) -> Result<(), TaskStoreError> {
        let mut file = File::create(self.tree_path(root_id))?;
        for task in self.tree(root_id) {
            writeln!(file, "{}", serde_json::to_string(task)?)?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestTaskStore;

    #[test]
    fn test_create_tree() {
        let mut tasks = TestTaskStore::new();

        let description = TaskUpdate { description: Some("Detailed description".to_string()), ..Default::default() };
        let task = tasks.store_mut().create(None, "Research topic X", description).unwrap();

        assert!(task.id.is_root());
        assert_eq!(task.title, "Research topic X");
        assert_eq!(task.description, Some("Detailed description".to_string()));
        assert_eq!(task.status, TaskStatus::Pending);

        let file_path = tasks.store().active_dir().join(task.id.filename());
        assert!(file_path.exists());
    }

    #[test]
    fn test_add_subtask() {
        let mut tasks = TestTaskStore::new();

        let root = tasks.root_task("Root task");
        let subtask = tasks.subtask(&root.id, "Subtask 1");

        assert!(!subtask.id.is_root());
        assert_eq!(subtask.parent, Some(root.id.clone()));
        assert_eq!(subtask.title, "Subtask 1");
        assert_eq!(tasks.store().tasks().count(), 2);
    }

    #[test]
    fn subtask_numbering_continues_across_the_tree_after_reopen() {
        let mut tasks = TestTaskStore::new();
        let root = tasks.root_task("Root");
        let first = tasks.subtask(&root.id, "First");
        let nested = tasks.subtask(&first.id, "Nested");

        tasks.reopen();
        let next = tasks.subtask(&root.id, "Next");

        assert_eq!(nested.id.as_str(), format!("{}.1.2", root.id));
        assert_eq!(next.id.as_str(), format!("{}.3", root.id));
    }

    #[test]
    fn test_update_task() {
        let mut tasks = TestTaskStore::new();
        let task = tasks.root_task("Original title");

        let updated = tasks
            .store_mut()
            .update(
                &task.id,
                TaskUpdate {
                    title: Some("Updated title".to_string()),
                    status: Some(TaskStatus::InProgress),
                    assignee: Some("worker-1".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();

        assert_eq!(updated.title, "Updated title");
        assert_eq!(updated.status, TaskStatus::InProgress);
        assert_eq!(updated.assignee, Some("worker-1".to_string()));
    }

    #[test]
    fn test_complete_task_with_summary() {
        let mut tasks = TestTaskStore::new();
        let task = tasks.root_task("Task to complete");

        let completed = tasks
            .store_mut()
            .update(
                &task.id,
                TaskUpdate {
                    status: Some(TaskStatus::Completed),
                    summary: Some("Found the answer!".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();

        assert_eq!(completed.status, TaskStatus::Completed);
        assert_eq!(completed.summary, Some("Found the answer!".to_string()));
    }

    #[test]
    fn test_is_ready() {
        let mut tasks = TestTaskStore::new();

        let root = tasks.root_task("Root");
        let subtask1 = tasks.subtask(&root.id, "Subtask 1");
        let subtask2 = tasks.subtask(&root.id, "Subtask 2");

        tasks
            .store_mut()
            .update(&subtask2.id, TaskUpdate { deps: Some(vec![subtask1.id.clone()]), ..Default::default() })
            .unwrap();

        assert_eq!(ready_titles(&tasks), ["Root", "Subtask 1"]);

        tasks
            .store_mut()
            .update(
                &subtask1.id,
                TaskUpdate {
                    status: Some(TaskStatus::Completed),
                    summary: Some("done".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();

        assert_eq!(ready_titles(&tasks), ["Root", "Subtask 2"]);
    }

    #[test]
    fn test_persistence() {
        let mut tasks = TestTaskStore::new();

        let task = tasks.root_task("Persistent task");
        tasks.subtask(&task.id, "Subtask 1");
        tasks.subtask(&task.id, "Subtask 2");

        tasks.reopen();

        assert_eq!(tasks.store().tasks().count(), 3);
    }

    #[test]
    fn test_error_task_not_found() {
        let tasks = TestTaskStore::new();

        let result = tasks.store().get(&TaskId::from("at-nonexistent"));
        assert!(result.is_none());
    }

    #[test]
    fn test_error_parent_not_found() {
        let mut tasks = TestTaskStore::new();

        let result = tasks.store_mut().create(Some(&TaskId::from("at-nonexistent")), "Orphan", TaskUpdate::default());
        assert!(matches!(result, Err(TaskStoreError::ParentNotFound { .. })));
    }

    #[test]
    fn test_error_dependency_not_found() {
        let mut tasks = TestTaskStore::new();
        let task = tasks.root_task("Task");

        let result = tasks
            .store_mut()
            .update(&task.id, TaskUpdate { deps: Some(vec![TaskId::from("at-nonexistent")]), ..Default::default() });

        assert!(matches!(result, Err(TaskStoreError::DependencyNotFound { .. })));
    }

    #[test]
    fn test_init_is_idempotent() {
        let mut tasks = TestTaskStore::new();
        let store = tasks.store_mut();

        let task = store.create(None, "Test task", TaskUpdate::default()).unwrap();
        assert_eq!(store.tasks().count(), 1);

        store.init().unwrap();
        assert_eq!(store.tasks().count(), 1);

        let retrieved = store.get(&task.id);
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().title, "Test task");
    }

    fn ready_titles(tasks: &TestTaskStore) -> Vec<&str> {
        let store = tasks.store();
        store.tasks().filter(|task| store.is_ready(task)).map(|task| task.title.as_str()).collect()
    }
}
