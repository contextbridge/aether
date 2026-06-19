use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::hash::Hash;

/// Task identifier with hierarchical support.
/// - Root tasks: `at-{hash8}` (e.g., `at-a1b2c3d4`)
/// - Subtasks: `at-{hash8}.{n}` (e.g., `at-a1b2c3d4.1`, `at-a1b2c3d4.2`)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct TaskId(String);

impl TaskId {
    /// Create a new root task ID from a hash
    pub fn new_root(hash: &str) -> Self {
        Self(format!("at-{hash}"))
    }

    /// Create a subtask ID from a parent and index
    pub fn new_subtask(parent: &TaskId, index: usize) -> Self {
        Self(format!("{}.{}", parent.0, index))
    }

    /// Get the root task ID (strips subtask suffix if present)
    pub fn root(&self) -> TaskId {
        if let Some(dot_pos) = self.0.find('.') { TaskId(self.0[..dot_pos].to_string()) } else { self.clone() }
    }

    /// Check if this is a root task (no parent)
    pub fn is_root(&self) -> bool {
        !self.0.contains('.')
    }

    /// Get the parent task ID if this is a subtask
    pub fn parent(&self) -> Option<TaskId> {
        self.0.rfind('.').map(|dot_pos| TaskId(self.0[..dot_pos].to_string()))
    }

    /// Get the raw string representation
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Generate the filename for this task tree (based on root ID)
    pub fn filename(&self) -> String {
        format!("{}.jsonl", self.root().0)
    }
}

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for TaskId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for TaskId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

/// Task status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum TaskStatus {
    Pending,
    #[serde(alias = "in_progress")]
    InProgress,
    Completed,
    Blocked,
}

impl TaskStatus {
    /// Returns the canonical camelCase string for wire format
    pub fn as_wire_str(&self) -> &'static str {
        match self {
            TaskStatus::Pending => "pending",
            TaskStatus::InProgress => "inProgress",
            TaskStatus::Completed => "completed",
            TaskStatus::Blocked => "blocked",
        }
    }
}

impl fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_wire_str())
    }
}

/// A task for tracking work in deep research workflows
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    /// Unique task identifier
    pub id: TaskId,

    /// Short descriptive title
    pub title: String,

    /// Optional detailed description (markdown)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Current status
    pub status: TaskStatus,

    /// Agent or worker assigned to this task
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,

    /// Parent task ID (for subtasks)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<TaskId>,

    /// Dependencies - task IDs that must complete before this starts
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deps: Vec<TaskId>,

    // === Result/handoff fields (flattened) ===
    /// Summary of work done (1-3 sentences)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,

    /// Key decisions made (what was decided and why)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decisions: Vec<String>,

    /// Important facts discovered (errors, patterns, constraints)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub facts: Vec<String>,

    /// Suggested next steps
    #[serde(default, skip_serializing_if = "Vec::is_empty", alias = "next_steps")]
    pub next_steps: Vec<String>,

    /// Blockers or unresolved issues
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blockers: Vec<String>,

    /// Files examined (not modified - git tracks modifications)
    #[serde(default, skip_serializing_if = "Vec::is_empty", alias = "files_read")]
    pub files_read: Vec<String>,

    /// External resources accessed with brief notes
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resources: Vec<String>,

    /// When the task was created
    #[serde(alias = "created_at")]
    pub created_at: DateTime<Utc>,

    /// When the task was last updated
    #[serde(alias = "updated_at")]
    pub updated_at: DateTime<Utc>,
}

impl Task {
    /// Create a new root task
    pub fn new_root(id: TaskId, title: String) -> Self {
        let now = Utc::now();
        Self {
            id,
            title,
            description: None,
            status: TaskStatus::Pending,
            assignee: None,
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

    /// Create a new subtask
    pub fn new_subtask(id: TaskId, parent: TaskId, title: String) -> Self {
        let now = Utc::now();
        Self {
            id,
            title,
            description: None,
            status: TaskStatus::Pending,
            assignee: None,
            parent: Some(parent),
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

    /// Check if the task can be started (all dependencies completed)
    pub fn is_ready(&self, completed_tasks: &[&TaskId]) -> bool {
        self.status == TaskStatus::Pending && self.deps.iter().all(|dep| completed_tasks.contains(&dep))
    }

    /// Update the task's `updated_at` timestamp
    pub fn touch(&mut self) {
        self.updated_at = Utc::now();
    }
}

/// Updates that can be applied to a task
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TaskUpdate {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<TaskStatus>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub deps: Option<Vec<TaskId>>,

    // === Result/handoff fields (flattened) ===
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub decisions: Option<Vec<String>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub facts: Option<Vec<String>>,

    #[serde(skip_serializing_if = "Option::is_none", alias = "next_steps")]
    pub next_steps: Option<Vec<String>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub blockers: Option<Vec<String>>,

    #[serde(skip_serializing_if = "Option::is_none", alias = "files_read")]
    pub files_read: Option<Vec<String>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<Vec<String>>,
}

impl TaskUpdate {
    /// Apply updates to a task, consuming the update
    pub fn apply_to(self, task: &mut Task) {
        if let Some(v) = self.title {
            task.title = v;
        }
        if let Some(v) = self.description {
            task.description = Some(v);
        }
        if let Some(v) = self.status {
            task.status = v;
        }
        if let Some(v) = self.assignee {
            task.assignee = Some(v);
        }
        if let Some(v) = self.deps {
            task.deps = v;
        }
        if let Some(v) = self.summary {
            task.summary = Some(v);
        }
        if let Some(v) = self.decisions {
            task.decisions = v;
        }
        if let Some(v) = self.facts {
            task.facts = v;
        }
        if let Some(v) = self.next_steps {
            task.next_steps = v;
        }
        if let Some(v) = self.blockers {
            task.blockers = v;
        }
        if let Some(v) = self.files_read {
            task.files_read = v;
        }
        if let Some(v) = self.resources {
            task.resources = v;
        }
        task.touch();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_id_root() {
        let id = TaskId::new_root("a1b2c3d4");
        assert_eq!(id.as_str(), "at-a1b2c3d4");
        assert!(id.is_root());
        assert_eq!(id.parent(), None);
        assert_eq!(id.filename(), "at-a1b2c3d4.jsonl");
    }

    #[test]
    fn test_task_id_subtask() {
        let root = TaskId::new_root("a1b2c3d4");
        let subtask = TaskId::new_subtask(&root, 1);
        assert_eq!(subtask.as_str(), "at-a1b2c3d4.1");
        assert!(!subtask.is_root());
        assert_eq!(subtask.parent(), Some(root.clone()));
        assert_eq!(subtask.root(), root);
        assert_eq!(subtask.filename(), "at-a1b2c3d4.jsonl");
    }

    #[test]
    fn test_task_id_nested_subtask() {
        let root = TaskId::new_root("a1b2c3d4");
        let subtask1 = TaskId::new_subtask(&root, 1);
        let subtask2 = TaskId::new_subtask(&subtask1, 2);
        assert_eq!(subtask2.as_str(), "at-a1b2c3d4.1.2");
        assert_eq!(subtask2.parent(), Some(subtask1));
        assert_eq!(subtask2.root(), root);
    }

    #[test]
    fn test_task_is_ready() {
        let id1 = TaskId::new_root("task1");
        let id2 = TaskId::new_root("task2");
        let id3 = TaskId::new_root("task3");

        let mut task = Task::new_root(id3.clone(), "Test Task".to_string());
        task.deps = vec![id1.clone(), id2.clone()];

        assert!(!task.is_ready(&[]));

        assert!(!task.is_ready(&[&id1]));

        assert!(task.is_ready(&[&id1, &id2]));

        task.status = TaskStatus::InProgress;
        assert!(!task.is_ready(&[&id1, &id2]));
    }

    #[test]
    fn test_task_update_apply() {
        let id = TaskId::new_root("test");
        let mut task = Task::new_root(id, "Original".to_string());

        let update = TaskUpdate {
            title: Some("Updated".to_string()),
            status: Some(TaskStatus::InProgress),
            assignee: Some("worker-1".to_string()),
            ..Default::default()
        };

        update.apply_to(&mut task);

        assert_eq!(task.title, "Updated");
        assert_eq!(task.status, TaskStatus::InProgress);
        assert_eq!(task.assignee, Some("worker-1".to_string()));
    }

    #[test]
    fn test_task_serialization() {
        let id = TaskId::new_root("a1b2c3d4");
        let task = Task::new_root(id, "Test Task".to_string());

        let json = serde_json::to_string(&task).unwrap();
        assert!(json.contains("\"id\":\"at-a1b2c3d4\""));
        assert!(json.contains("\"status\":\"pending\""));
        // Verify camelCase field names
        assert!(json.contains("\"createdAt\""));
        assert!(json.contains("\"updatedAt\""));

        let deserialized: Task = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id.as_str(), "at-a1b2c3d4");
        assert_eq!(deserialized.status, TaskStatus::Pending);
    }

    #[test]
    fn test_task_deserializes_legacy_snake_case() {
        // Test backward compatibility with legacy snake_case JSON
        let legacy_json = r#"{
            "id": "at-legacy",
            "title": "Legacy Task",
            "status": "pending",
            "created_at": "2024-01-01T00:00:00Z",
            "updated_at": "2024-01-01T00:00:00Z",
            "next_steps": ["step1"],
            "files_read": ["file.rs"]
        }"#;
        let task: Task = serde_json::from_str(legacy_json).unwrap();
        assert_eq!(task.id.as_str(), "at-legacy");
        assert_eq!(task.next_steps, vec!["step1"]);
        assert_eq!(task.files_read, vec!["file.rs"]);
    }

    #[test]
    fn test_task_status_serialization() {
        // Test that TaskStatus serializes to camelCase
        assert_eq!(serde_json::to_string(&TaskStatus::Pending).unwrap(), "\"pending\"");
        assert_eq!(serde_json::to_string(&TaskStatus::InProgress).unwrap(), "\"inProgress\"");
        assert_eq!(serde_json::to_string(&TaskStatus::Completed).unwrap(), "\"completed\"");
        assert_eq!(serde_json::to_string(&TaskStatus::Blocked).unwrap(), "\"blocked\"");
    }

    #[test]
    fn test_task_status_deserialization_aliases() {
        // Test that both camelCase and snake_case are accepted
        let camel: TaskStatus = serde_json::from_str("\"inProgress\"").unwrap();
        let snake: TaskStatus = serde_json::from_str("\"in_progress\"").unwrap();
        assert_eq!(camel, TaskStatus::InProgress);
        assert_eq!(snake, TaskStatus::InProgress);
    }

    #[test]
    fn test_task_status_wire_str() {
        assert_eq!(TaskStatus::Pending.as_wire_str(), "pending");
        assert_eq!(TaskStatus::InProgress.as_wire_str(), "inProgress");
        assert_eq!(TaskStatus::Completed.as_wire_str(), "completed");
        assert_eq!(TaskStatus::Blocked.as_wire_str(), "blocked");
    }

    #[test]
    fn test_task_with_flattened_fields() {
        let id = TaskId::new_root("a1b2c3d4");
        let mut task = Task::new_root(id, "Research task".to_string());

        task.summary = Some("Found the answer".to_string());
        task.decisions = vec!["Chose option A".to_string()];
        task.facts = vec!["Key discovery".to_string()];
        task.next_steps = vec!["Do X next".to_string()];

        let json = serde_json::to_string(&task).unwrap();
        assert!(json.contains("\"summary\":\"Found the answer\""));
        assert!(json.contains("\"decisions\":["));
        assert!(json.contains("\"facts\":["));
        // Should serialize as camelCase
        assert!(json.contains("\"nextSteps\":["));
        // Should NOT contain snake_case
        assert!(!json.contains("\"next_steps\""));

        let deserialized: Task = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.summary, Some("Found the answer".to_string()));
        assert_eq!(deserialized.facts.len(), 1);
    }

    #[test]
    fn test_task_empty_fields_omitted() {
        let id = TaskId::new_root("a1b2c3d4");
        let task = Task::new_root(id, "Test Task".to_string());

        let json = serde_json::to_string(&task).unwrap();

        // Empty fields should be omitted
        assert!(!json.contains("\"summary\""));
        assert!(!json.contains("\"decisions\""));
        assert!(!json.contains("\"facts\""));
        assert!(!json.contains("\"nextSteps\""));
        assert!(!json.contains("\"blockers\""));
        assert!(!json.contains("\"filesRead\""));
        assert!(!json.contains("\"resources\""));
    }

    #[test]
    fn test_task_update_with_flattened_fields() {
        let id = TaskId::new_root("test");
        let mut task = Task::new_root(id, "Original".to_string());

        let update = TaskUpdate {
            status: Some(TaskStatus::Completed),
            summary: Some("Task done".to_string()),
            facts: Some(vec!["Found X".to_string()]),
            ..Default::default()
        };

        update.apply_to(&mut task);

        assert_eq!(task.status, TaskStatus::Completed);
        assert_eq!(task.summary, Some("Task done".to_string()));
        assert_eq!(task.facts, vec!["Found X".to_string()]);
    }
}
