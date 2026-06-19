use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::common::{TaskSummary, task_result_meta};
use crate::tasks::task_store::{TaskStore, TaskStoreError};
use crate::tasks::types::{TaskId, TaskStatus, TaskUpdate};
use mcp_utils::display_meta::ToolResultMeta;

/// Input for the `task_update` tool
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TaskUpdateInput {
    /// Task ID to update
    pub id: String,

    #[serde(default)]
    pub title: Option<String>,

    #[serde(default)]
    pub description: Option<String>,

    /// Status: pending, `in_progress`, completed, or blocked
    #[serde(default)]
    pub status: Option<TaskStatus>,

    #[serde(default)]
    pub assignee: Option<String>,

    #[serde(default)]
    pub deps: Option<Vec<String>>,

    #[serde(default)]
    pub summary: Option<String>,

    #[serde(default)]
    pub decisions: Option<Vec<String>>,

    #[serde(default)]
    pub facts: Option<Vec<String>>,

    #[serde(default, alias = "next_steps")]
    pub next_steps: Option<Vec<String>>,

    #[serde(default)]
    pub blockers: Option<Vec<String>>,

    #[serde(default, alias = "files_read")]
    pub files_read: Option<Vec<String>>,

    #[serde(default)]
    pub resources: Option<Vec<String>>,
}

/// Output for the `task_update` tool
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TaskUpdateOutput {
    /// Status of the operation
    pub status: String,

    /// The updated task
    pub task: TaskSummary,

    /// Human-readable message
    pub message: String,

    /// Fields that were changed
    pub changes: Vec<String>,

    /// Tasks that are now ready to start (when completing a task that others depended on)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub newly_ready: Vec<TaskSummary>,

    /// Display metadata for human-friendly rendering
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub meta: Option<ToolResultMeta>,
}

/// Update an existing task's fields
pub fn execute_task_update(input: TaskUpdateInput, store: &mut TaskStore) -> Result<TaskUpdateOutput, TaskStoreError> {
    let id = TaskId::from(input.id.as_str());

    let changes: Vec<String> = [
        input.title.as_ref().map(|_| "title"),
        input.description.as_ref().map(|_| "description"),
        input.status.as_ref().map(|_| "status"),
        input.assignee.as_ref().map(|_| "assignee"),
        input.deps.as_ref().map(|_| "deps"),
        input.summary.as_ref().map(|_| "summary"),
        input.decisions.as_ref().map(|_| "decisions"),
        input.facts.as_ref().map(|_| "facts"),
        input.next_steps.as_ref().map(|_| "next_steps"),
        input.blockers.as_ref().map(|_| "blockers"),
        input.files_read.as_ref().map(|_| "files_read"),
        input.resources.as_ref().map(|_| "resources"),
    ]
    .into_iter()
    .flatten()
    .map(String::from)
    .collect();

    let ready_before: Vec<String> = if input.status == Some(TaskStatus::Completed) {
        store.get_ready().iter().map(|t| t.id.to_string()).collect()
    } else {
        vec![]
    };

    let update = TaskUpdate {
        title: input.title,
        description: input.description,
        status: input.status,
        assignee: input.assignee,
        deps: input.deps.map(|d| d.iter().map(|s| TaskId::from(s.as_str())).collect()),
        summary: input.summary,
        decisions: input.decisions,
        facts: input.facts,
        next_steps: input.next_steps,
        blockers: input.blockers,
        files_read: input.files_read,
        resources: input.resources,
    };

    let task = store.update(&id, update)?;

    let newly_ready: Vec<TaskSummary> = if task.status == TaskStatus::Completed {
        store
            .get_ready()
            .iter()
            .filter(|t| !ready_before.contains(&t.id.to_string()))
            .map(|t| TaskSummary::from(*t))
            .collect()
    } else {
        vec![]
    };

    let message = if changes.is_empty() {
        format!("No changes made to task {id}")
    } else if task.status == TaskStatus::Completed {
        if newly_ready.is_empty() {
            format!("Completed task '{}'", task.title)
        } else if newly_ready.len() == 1 {
            format!("Completed task '{}'. 1 task is now ready: {}", task.title, newly_ready[0].title)
        } else {
            format!("Completed task '{}'. {} tasks are now ready", task.title, newly_ready.len())
        }
    } else {
        format!("Updated {} on task {}", changes.join(", "), id)
    };

    Ok(TaskUpdateOutput {
        status: "success".to_string(),
        task: TaskSummary::from(&task),
        message,
        changes,
        newly_ready,
        meta: Some(task_result_meta(store, &task.title)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup() -> (TempDir, TaskStore) {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path().join(".aether-tasks");
        let mut store = TaskStore::new(root);
        store.init().unwrap();
        (temp_dir, store)
    }

    #[test]
    fn test_update_title() {
        let (_temp, mut store) = setup();

        let task = store.create_tree("Original", None).unwrap();

        let input =
            TaskUpdateInput { id: task.id.to_string(), title: Some("Updated title".to_string()), ..Default::default() };

        let output = execute_task_update(input, &mut store).unwrap();

        assert_eq!(output.status, "success");
        assert_eq!(output.task.title, "Updated title");
        assert_eq!(output.changes, vec!["title"]);
    }

    #[test]
    fn test_update_status() {
        let (_temp, mut store) = setup();

        let task = store.create_tree("Task", None).unwrap();

        let input =
            TaskUpdateInput { id: task.id.to_string(), status: Some(TaskStatus::InProgress), ..Default::default() };

        let output = execute_task_update(input, &mut store).unwrap();

        assert_eq!(output.task.status, "inProgress");
        assert_eq!(output.changes, vec!["status"]);
    }

    #[test]
    fn test_update_multiple_fields() {
        let (_temp, mut store) = setup();

        let task = store.create_tree("Task", None).unwrap();

        let input = TaskUpdateInput {
            id: task.id.to_string(),
            title: Some("New title".to_string()),
            description: Some("New description".to_string()),
            status: Some(TaskStatus::InProgress),
            assignee: Some("worker-1".to_string()),
            ..Default::default()
        };

        let output = execute_task_update(input, &mut store).unwrap();

        assert_eq!(output.changes.len(), 4);
        assert!(output.changes.contains(&"title".to_string()));
        assert!(output.changes.contains(&"description".to_string()));
        assert!(output.changes.contains(&"status".to_string()));
        assert!(output.changes.contains(&"assignee".to_string()));
    }

    #[test]
    fn test_update_nonexistent_task() {
        let (_temp, mut store) = setup();

        let input = TaskUpdateInput {
            id: "at-nonexistent".to_string(),
            title: Some("New title".to_string()),
            ..Default::default()
        };

        let result = execute_task_update(input, &mut store);
        assert!(result.is_err());
    }

    #[test]
    fn test_update_with_invalid_deps() {
        let (_temp, mut store) = setup();

        let task = store.create_tree("Task", None).unwrap();

        let input = TaskUpdateInput {
            id: task.id.to_string(),
            deps: Some(vec!["at-nonexistent".to_string()]),
            ..Default::default()
        };

        let result = execute_task_update(input, &mut store);
        assert!(result.is_err());
    }

    #[test]
    fn test_complete_task() {
        let (_temp, mut store) = setup();

        let task = store.create_tree("Task to complete", None).unwrap();

        let input = TaskUpdateInput {
            id: task.id.to_string(),
            status: Some(TaskStatus::Completed),
            summary: Some("Task done successfully".to_string()),
            ..Default::default()
        };

        let output = execute_task_update(input, &mut store).unwrap();

        assert_eq!(output.task.status, "completed");
        assert!(output.message.contains("Completed task"));
    }

    #[test]
    fn test_complete_unblocks_dependent() {
        let (_temp, mut store) = setup();

        let root = store.create_tree("Root", None).unwrap();
        let sub1 = store.add_subtask(&root.id, "Subtask 1").unwrap();
        let sub2 = store.add_subtask(&root.id, "Subtask 2").unwrap();

        store.update(&sub2.id, TaskUpdate { deps: Some(vec![sub1.id.clone()]), ..Default::default() }).unwrap();

        let input = TaskUpdateInput {
            id: sub1.id.to_string(),
            status: Some(TaskStatus::Completed),
            summary: Some("Subtask 1 done".to_string()),
            ..Default::default()
        };

        let output = execute_task_update(input, &mut store).unwrap();

        assert_eq!(output.newly_ready.len(), 1);
        assert_eq!(output.newly_ready[0].id, sub2.id.to_string());
        assert!(output.message.contains("1 task is now ready"));
    }

    #[test]
    fn test_complete_with_flat_fields() {
        let (_temp, mut store) = setup();

        let task = store.create_tree("Research task", None).unwrap();

        let input = TaskUpdateInput {
            id: task.id.to_string(),
            status: Some(TaskStatus::Completed),
            summary: Some("Found 3 issues".to_string()),
            decisions: Some(vec!["Use approach A".to_string()]),
            facts: Some(vec!["Issue in file X".to_string()]),
            next_steps: Some(vec!["Fix the issues".to_string()]),
            ..Default::default()
        };

        let output = execute_task_update(input, &mut store).unwrap();

        assert_eq!(output.task.status, "completed");
        assert!(output.changes.contains(&"summary".to_string()));
        assert!(output.changes.contains(&"decisions".to_string()));
        assert!(output.changes.contains(&"facts".to_string()));
    }
}
