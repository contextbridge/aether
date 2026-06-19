use super::types::{Task, TaskId, TaskStatus};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

/// In-memory index for fast task queries.
/// Rebuilt on startup by scanning the active directory.
#[derive(Debug, Default)]
pub struct TaskIndex {
    /// All tasks by ID
    tasks: HashMap<TaskId, Task>,

    /// Tasks grouped by assignee
    by_assignee: HashMap<String, HashSet<TaskId>>,

    /// Tasks grouped by status
    by_status: HashMap<TaskStatus, HashSet<TaskId>>,

    /// Root task ID -> file path mapping
    trees: HashMap<TaskId, PathBuf>,

    /// Next subtask index for each root task
    subtask_counters: HashMap<TaskId, usize>,
}

impl TaskIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a task to the index
    pub fn insert(&mut self, task: Task, file_path: PathBuf) {
        let id = task.id.clone();
        let root_id = id.root();

        self.trees.entry(root_id.clone()).or_insert(file_path);

        if !id.is_root()
            && let Some(suffix) = id.as_str().rsplit('.').next()
            && let Ok(idx) = suffix.parse::<usize>()
        {
            let counter = self.subtask_counters.entry(root_id).or_insert(0);
            *counter = (*counter).max(idx);
        }

        if let Some(assignee) = &task.assignee {
            self.by_assignee.entry(assignee.clone()).or_default().insert(id.clone());
        }

        self.by_status.entry(task.status).or_default().insert(id.clone());

        self.tasks.insert(id, task);
    }

    /// Update a task in the index (re-indexes if assignee/status changed)
    pub fn update(&mut self, task: Task) {
        let id = task.id.clone();

        if let Some(old_task) = self.tasks.get(&id) {
            if let Some(old_assignee) = &old_task.assignee
                && let Some(ids) = self.by_assignee.get_mut(old_assignee)
            {
                ids.remove(&id);
                if ids.is_empty() {
                    self.by_assignee.remove(old_assignee);
                }
            }

            if let Some(ids) = self.by_status.get_mut(&old_task.status) {
                ids.remove(&id);
                if ids.is_empty() {
                    self.by_status.remove(&old_task.status);
                }
            }
        }

        if let Some(assignee) = &task.assignee {
            self.by_assignee.entry(assignee.clone()).or_default().insert(id.clone());
        }

        self.by_status.entry(task.status).or_default().insert(id.clone());

        self.tasks.insert(id, task);
    }

    /// Remove a task from the index
    pub fn remove(&mut self, id: &TaskId) -> Option<Task> {
        if let Some(task) = self.tasks.remove(id) {
            if let Some(assignee) = &task.assignee
                && let Some(ids) = self.by_assignee.get_mut(assignee)
            {
                ids.remove(id);
                if ids.is_empty() {
                    self.by_assignee.remove(assignee);
                }
            }

            if let Some(ids) = self.by_status.get_mut(&task.status) {
                ids.remove(id);
                if ids.is_empty() {
                    self.by_status.remove(&task.status);
                }
            }

            Some(task)
        } else {
            None
        }
    }

    /// Remove an entire task tree from the index
    pub fn remove_tree(&mut self, root_id: &TaskId) -> Vec<Task> {
        let mut removed = Vec::new();

        let ids_to_remove: Vec<TaskId> = self.tasks.keys().filter(|id| id.root() == *root_id).cloned().collect();

        for id in ids_to_remove {
            if let Some(task) = self.remove(&id) {
                removed.push(task);
            }
        }

        self.trees.remove(root_id);
        self.subtask_counters.remove(root_id);

        removed
    }

    /// Get a task by ID
    pub fn get(&self, id: &TaskId) -> Option<&Task> {
        self.tasks.get(id)
    }

    /// Get all tasks in a tree
    pub fn get_tree(&self, root_id: &TaskId) -> Vec<&Task> {
        self.tasks.values().filter(|task| task.id.root() == *root_id).collect()
    }

    /// Get the file path for a task tree
    pub fn get_tree_path(&self, root_id: &TaskId) -> Option<&PathBuf> {
        self.trees.get(root_id)
    }

    /// Get all tasks for an assignee
    pub fn get_by_assignee(&self, assignee: &str) -> Vec<&Task> {
        self.by_assignee
            .get(assignee)
            .map(|ids| ids.iter().filter_map(|id| self.tasks.get(id)).collect())
            .unwrap_or_default()
    }

    /// Get all tasks with a specific status
    pub fn get_by_status(&self, status: TaskStatus) -> Vec<&Task> {
        self.by_status
            .get(&status)
            .map(|ids| ids.iter().filter_map(|id| self.tasks.get(id)).collect())
            .unwrap_or_default()
    }

    /// Get all tasks that are ready to start (pending with all deps completed)
    pub fn get_ready(&self) -> Vec<&Task> {
        let completed_ids: Vec<&TaskId> =
            self.by_status.get(&TaskStatus::Completed).map(|ids| ids.iter().collect()).unwrap_or_default();

        self.by_status
            .get(&TaskStatus::Pending)
            .map(|ids| {
                ids.iter().filter_map(|id| self.tasks.get(id)).filter(|task| task.is_ready(&completed_ids)).collect()
            })
            .unwrap_or_default()
    }

    /// Get the next subtask index for a root task
    pub fn next_subtask_index(&mut self, root_id: &TaskId) -> usize {
        let counter = self.subtask_counters.entry(root_id.clone()).or_insert(0);
        *counter += 1;
        *counter
    }

    /// Get all root task IDs
    pub fn root_ids(&self) -> Vec<&TaskId> {
        self.trees.keys().collect()
    }

    /// Get all tasks (for iteration)
    pub fn all_tasks(&self) -> impl Iterator<Item = &Task> {
        self.tasks.values()
    }

    /// Check if a task exists
    pub fn contains(&self, id: &TaskId) -> bool {
        self.tasks.contains_key(id)
    }

    /// Get the total number of tasks
    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    /// Check if the index is empty
    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    /// Clear the index
    pub fn clear(&mut self) {
        self.tasks.clear();
        self.by_assignee.clear();
        self.by_status.clear();
        self.trees.clear();
        self.subtask_counters.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_task(id: &str, status: TaskStatus, assignee: Option<&str>) -> Task {
        let now = Utc::now();
        Task {
            id: TaskId::from(id),
            title: format!("Task {id}"),
            description: None,
            status,
            assignee: assignee.map(String::from),
            parent: None,
            deps: Vec::new(),
            summary: None,
            decisions: Vec::new(),
            facts: Vec::new(),
            next_steps: Vec::new(),
            blockers: Vec::new(),
            files_read: Vec::new(),
            resources: Vec::new(),
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn test_insert_and_get() {
        let mut index = TaskIndex::new();
        let task = make_task("at-abc123", TaskStatus::Pending, Some("worker-1"));
        let path = PathBuf::from(".aether-tasks/active/at-abc123.jsonl");

        index.insert(task.clone(), path.clone());

        assert!(index.contains(&TaskId::from("at-abc123")));
        assert_eq!(index.len(), 1);

        let retrieved = index.get(&TaskId::from("at-abc123")).unwrap();
        assert_eq!(retrieved.title, "Task at-abc123");
    }

    #[test]
    fn test_index_by_assignee() {
        let mut index = TaskIndex::new();
        let path = PathBuf::from(".aether-tasks/active/at-abc123.jsonl");

        index.insert(make_task("at-abc123", TaskStatus::Pending, Some("worker-1")), path.clone());
        index.insert(make_task("at-abc123.1", TaskStatus::InProgress, Some("worker-1")), path.clone());
        index.insert(make_task("at-abc123.2", TaskStatus::Pending, Some("worker-2")), path);

        let worker1_tasks = index.get_by_assignee("worker-1");
        assert_eq!(worker1_tasks.len(), 2);

        let worker2_tasks = index.get_by_assignee("worker-2");
        assert_eq!(worker2_tasks.len(), 1);
    }

    #[test]
    fn test_index_by_status() {
        let mut index = TaskIndex::new();
        let path = PathBuf::from(".aether-tasks/active/at-abc123.jsonl");

        index.insert(make_task("at-abc123", TaskStatus::Pending, None), path.clone());
        index.insert(make_task("at-abc123.1", TaskStatus::InProgress, None), path.clone());
        index.insert(make_task("at-abc123.2", TaskStatus::Completed, None), path);

        assert_eq!(index.get_by_status(TaskStatus::Pending).len(), 1);
        assert_eq!(index.get_by_status(TaskStatus::InProgress).len(), 1);
        assert_eq!(index.get_by_status(TaskStatus::Completed).len(), 1);
        assert_eq!(index.get_by_status(TaskStatus::Blocked).len(), 0);
    }

    #[test]
    fn test_update_reindexes() {
        let mut index = TaskIndex::new();
        let path = PathBuf::from(".aether-tasks/active/at-abc123.jsonl");

        let mut task = make_task("at-abc123", TaskStatus::Pending, Some("worker-1"));
        index.insert(task.clone(), path);

        task.status = TaskStatus::InProgress;
        task.assignee = Some("worker-2".to_string());
        index.update(task);

        assert!(index.get_by_assignee("worker-1").is_empty());
        assert!(index.get_by_status(TaskStatus::Pending).is_empty());

        assert_eq!(index.get_by_assignee("worker-2").len(), 1);
        assert_eq!(index.get_by_status(TaskStatus::InProgress).len(), 1);
    }

    #[test]
    fn test_get_tree() {
        let mut index = TaskIndex::new();
        let path = PathBuf::from(".aether-tasks/active/at-abc123.jsonl");

        index.insert(make_task("at-abc123", TaskStatus::Pending, None), path.clone());
        index.insert(make_task("at-abc123.1", TaskStatus::Pending, None), path.clone());
        index.insert(make_task("at-abc123.2", TaskStatus::Pending, None), path);

        let tree = index.get_tree(&TaskId::from("at-abc123"));
        assert_eq!(tree.len(), 3);
    }

    #[test]
    fn test_remove_tree() {
        let mut index = TaskIndex::new();
        let path = PathBuf::from(".aether-tasks/active/at-abc123.jsonl");

        index.insert(make_task("at-abc123", TaskStatus::Pending, None), path.clone());
        index.insert(make_task("at-abc123.1", TaskStatus::Pending, None), path.clone());
        index.insert(make_task("at-abc123.2", TaskStatus::Pending, None), path);

        let removed = index.remove_tree(&TaskId::from("at-abc123"));
        assert_eq!(removed.len(), 3);
        assert!(index.is_empty());
    }

    #[test]
    fn test_get_ready() {
        let mut index = TaskIndex::new();
        let path = PathBuf::from(".aether-tasks/active/at-abc123.jsonl");

        index.insert(make_task("at-abc123", TaskStatus::Pending, None), path.clone());

        let mut task2 = make_task("at-abc123.2", TaskStatus::Pending, None);
        task2.deps = vec![TaskId::from("at-abc123.1")];
        index.insert(task2, path.clone());

        index.insert(make_task("at-abc123.1", TaskStatus::Completed, None), path);

        let ready = index.get_ready();
        assert_eq!(ready.len(), 2);

        let ready_ids: Vec<&str> = ready.iter().map(|t| t.id.as_str()).collect();
        assert!(ready_ids.contains(&"at-abc123"));
        assert!(ready_ids.contains(&"at-abc123.2"));
    }

    #[test]
    fn test_subtask_counter() {
        let mut index = TaskIndex::new();
        let path = PathBuf::from(".aether-tasks/active/at-abc123.jsonl");
        let root_id = TaskId::from("at-abc123");

        index.insert(make_task("at-abc123", TaskStatus::Pending, None), path);

        assert_eq!(index.next_subtask_index(&root_id), 1);
        assert_eq!(index.next_subtask_index(&root_id), 2);
        assert_eq!(index.next_subtask_index(&root_id), 3);
    }

    #[test]
    fn test_subtask_counter_from_existing() {
        let mut index = TaskIndex::new();
        let path = PathBuf::from(".aether-tasks/active/at-abc123.jsonl");
        let root_id = TaskId::from("at-abc123");

        index.insert(make_task("at-abc123", TaskStatus::Pending, None), path.clone());
        index.insert(make_task("at-abc123.5", TaskStatus::Pending, None), path.clone());

        assert_eq!(index.next_subtask_index(&root_id), 6);
    }
}
