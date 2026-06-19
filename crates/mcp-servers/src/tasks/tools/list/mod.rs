use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::common::TaskSummary;
use crate::tasks::task_store::TaskStore;
use crate::tasks::types::{TaskId, TaskStatus};
use mcp_utils::display_meta::{ToolDisplayMeta, ToolResultMeta};

/// Input for the `task_list` tool
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TaskListInput {
    /// Filter by assignee
    #[serde(default)]
    pub assignee: Option<String>,

    /// Filter by status: pending, `in_progress`, completed, or blocked
    #[serde(default)]
    pub status: Option<TaskStatusFilter>,

    /// List all tasks in a specific tree (by root task ID)
    #[serde(default, alias = "tree_id")]
    pub tree_id: Option<String>,

    /// Only return tasks that are ready to start (pending with all deps completed)
    #[serde(default, alias = "ready_only")]
    pub ready_only: Option<bool>,
}

/// Status filter for task listing
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum TaskStatusFilter {
    Pending,
    #[serde(alias = "in_progress")]
    InProgress,
    Completed,
    Blocked,
}

impl From<TaskStatusFilter> for TaskStatus {
    fn from(status: TaskStatusFilter) -> Self {
        match status {
            TaskStatusFilter::Pending => TaskStatus::Pending,
            TaskStatusFilter::InProgress => TaskStatus::InProgress,
            TaskStatusFilter::Completed => TaskStatus::Completed,
            TaskStatusFilter::Blocked => TaskStatus::Blocked,
        }
    }
}

/// Output for the `task_list` tool
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TaskListOutput {
    /// Status of the operation
    pub status: String,

    /// Matching tasks
    pub tasks: Vec<TaskSummary>,

    /// Total count of matching tasks
    pub count: usize,

    /// Human-readable message
    pub message: String,

    /// Display metadata for human-friendly rendering
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub meta: Option<ToolResultMeta>,
}

pub fn execute_task_list(input: &TaskListInput, store: &TaskStore) -> TaskListOutput {
    let tasks: Vec<TaskSummary> = if input.ready_only.unwrap_or(false) {
        store.get_ready().into_iter().map(TaskSummary::from).collect()
    } else if let Some(tree_id) = &input.tree_id {
        let tree_id = TaskId::from(tree_id.as_str());
        store.get_tree(&tree_id).map(|tasks| tasks.into_iter().map(TaskSummary::from).collect()).unwrap_or_default()
    } else if let Some(assignee) = &input.assignee {
        store.list_by_assignee(assignee).into_iter().map(TaskSummary::from).collect()
    } else if let Some(status) = input.status {
        store.list_by_status(status.into()).into_iter().map(TaskSummary::from).collect()
    } else {
        store
            .list_trees()
            .iter()
            .flat_map(|root_id| store.get_tree(root_id).unwrap_or_default().into_iter().map(TaskSummary::from))
            .collect()
    };

    let count = tasks.len();
    let filter_desc = build_filter_description(input);

    let message = if count == 0 {
        format!("No tasks found{filter_desc}")
    } else if count == 1 {
        format!("Found 1 task{filter_desc}")
    } else {
        format!("Found {count} tasks{filter_desc}")
    };

    let done = tasks.iter().filter(|t| t.status == "completed").count();
    let display_meta = ToolDisplayMeta::new("Todo", format!("{done}/{count} tasks"));

    TaskListOutput { status: "success".to_string(), tasks, count, message, meta: Some(display_meta.into()) }
}

fn build_filter_description(input: &TaskListInput) -> String {
    let mut parts = Vec::new();

    if input.ready_only.unwrap_or(false) {
        parts.push("ready to start".to_string());
    }
    if let Some(tree_id) = &input.tree_id {
        parts.push(format!("in tree {tree_id}"));
    }
    if let Some(assignee) = &input.assignee {
        parts.push(format!("assigned to {assignee}"));
    }
    if let Some(status) = input.status {
        let status_str = match status {
            TaskStatusFilter::Pending => "pending",
            TaskStatusFilter::InProgress => "inProgress",
            TaskStatusFilter::Completed => "completed",
            TaskStatusFilter::Blocked => "blocked",
        };
        parts.push(format!("with status {status_str}"));
    }

    if parts.is_empty() { String::new() } else { format!(" ({})", parts.join(", ")) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::TaskUpdate;
    use tempfile::TempDir;

    fn setup() -> (TempDir, TaskStore) {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path().join(".aether-tasks");
        let mut store = TaskStore::new(root);
        store.init().unwrap();
        (temp_dir, store)
    }

    #[test]
    fn test_list_all_tasks() {
        let (_temp, mut store) = setup();

        let root = store.create_tree("Root", None).unwrap();
        store.add_subtask(&root.id, "Subtask 1").unwrap();
        store.add_subtask(&root.id, "Subtask 2").unwrap();

        let output =
            execute_task_list(&TaskListInput { assignee: None, status: None, tree_id: None, ready_only: None }, &store);

        assert_eq!(output.count, 3);
        assert!(output.message.contains("Found 3 tasks"));
    }

    #[test]
    fn test_list_by_tree() {
        let (_temp, mut store) = setup();

        let root1 = store.create_tree("Root 1", None).unwrap();
        store.add_subtask(&root1.id, "Subtask 1.1").unwrap();

        let root2 = store.create_tree("Root 2", None).unwrap();
        store.add_subtask(&root2.id, "Subtask 2.1").unwrap();
        store.add_subtask(&root2.id, "Subtask 2.2").unwrap();

        let output = execute_task_list(
            &TaskListInput { assignee: None, status: None, tree_id: Some(root2.id.to_string()), ready_only: None },
            &store,
        );

        assert_eq!(output.count, 3);
        assert!(output.message.contains("in tree"));
    }

    #[test]
    fn test_list_by_assignee() {
        let (_temp, mut store) = setup();

        let root = store.create_tree("Root", None).unwrap();
        let sub1 = store.add_subtask(&root.id, "Task 1").unwrap();
        let sub2 = store.add_subtask(&root.id, "Task 2").unwrap();

        store.update(&sub1.id, TaskUpdate { assignee: Some("worker-1".to_string()), ..Default::default() }).unwrap();
        store.update(&sub2.id, TaskUpdate { assignee: Some("worker-2".to_string()), ..Default::default() }).unwrap();

        let output = execute_task_list(
            &TaskListInput { assignee: Some("worker-1".to_string()), status: None, tree_id: None, ready_only: None },
            &store,
        );

        assert_eq!(output.count, 1);
        assert_eq!(output.tasks[0].assignee, Some("worker-1".to_string()));
    }

    #[test]
    fn test_list_by_status() {
        let (_temp, mut store) = setup();

        let root = store.create_tree("Root", None).unwrap();
        let sub = store.add_subtask(&root.id, "Subtask").unwrap();

        store.update(&sub.id, TaskUpdate { status: Some(TaskStatus::InProgress), ..Default::default() }).unwrap();

        let output = execute_task_list(
            &TaskListInput {
                assignee: None,
                status: Some(TaskStatusFilter::InProgress),
                tree_id: None,
                ready_only: None,
            },
            &store,
        );

        assert_eq!(output.count, 1);
        assert_eq!(output.tasks[0].status, "inProgress");
    }

    #[test]
    fn test_list_ready_only() {
        let (_temp, mut store) = setup();

        let root = store.create_tree("Root", None).unwrap();
        let sub1 = store.add_subtask(&root.id, "Subtask 1").unwrap();
        let sub2 = store.add_subtask(&root.id, "Subtask 2").unwrap();

        store.update(&sub2.id, TaskUpdate { deps: Some(vec![sub1.id.clone()]), ..Default::default() }).unwrap();

        let output = execute_task_list(
            &TaskListInput { assignee: None, status: None, tree_id: None, ready_only: Some(true) },
            &store,
        );

        assert_eq!(output.count, 2);

        store
            .update(
                &sub1.id,
                TaskUpdate {
                    status: Some(TaskStatus::Completed),
                    summary: Some("done".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();

        let output = execute_task_list(
            &TaskListInput { assignee: None, status: None, tree_id: None, ready_only: Some(true) },
            &store,
        );

        assert_eq!(output.count, 2);
        let ready_ids: Vec<_> = output.tasks.iter().map(|t| t.id.as_str()).collect();
        assert!(ready_ids.contains(&sub2.id.as_str()));
    }

    #[test]
    fn test_list_empty() {
        let (_temp, store) = setup();

        let output =
            execute_task_list(&TaskListInput { assignee: None, status: None, tree_id: None, ready_only: None }, &store);

        assert_eq!(output.count, 0);
        assert!(output.message.contains("No tasks found"));
    }

    #[test]
    fn test_list_nonexistent_tree() {
        let (_temp, store) = setup();

        let output = execute_task_list(
            &TaskListInput {
                assignee: None,
                status: None,
                tree_id: Some("at-nonexistent".to_string()),
                ready_only: None,
            },
            &store,
        );

        assert_eq!(output.count, 0);
    }
}
