use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::common::TaskSummary;
use crate::tasks::task_store::{TaskStore, TaskStoreError};
use crate::tasks::types::{TaskId, TaskStatus, TaskUpdate};

/// Input for the `task_update` tool
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct TaskUpdateInput {
    /// Task ID to update
    pub id: String,

    #[serde(flatten)]
    pub update: TaskUpdate,
}

/// Output for the `task_update` tool
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TaskUpdateOutput {
    /// The updated task
    pub task: TaskSummary,

    /// Fields that were changed
    pub changes: Vec<String>,

    /// Tasks that are now ready to start (when completing a task that others depended on)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub newly_ready: Vec<TaskSummary>,
}

/// Update an existing task's fields
pub fn execute_task_update(input: TaskUpdateInput, store: &mut TaskStore) -> Result<TaskUpdateOutput, TaskStoreError> {
    let TaskUpdateInput { id, update } = input;
    let id = TaskId::from(id);

    let changes: Vec<String> = [
        update.title.as_ref().map(|_| "title"),
        update.description.as_ref().map(|_| "description"),
        update.status.as_ref().map(|_| "status"),
        update.assignee.as_ref().map(|_| "assignee"),
        update.deps.as_ref().map(|_| "deps"),
        update.summary.as_ref().map(|_| "summary"),
        update.decisions.as_ref().map(|_| "decisions"),
        update.facts.as_ref().map(|_| "facts"),
        update.next_steps.as_ref().map(|_| "next_steps"),
        update.blockers.as_ref().map(|_| "blockers"),
        update.files_read.as_ref().map(|_| "files_read"),
        update.resources.as_ref().map(|_| "resources"),
    ]
    .into_iter()
    .flatten()
    .map(String::from)
    .collect();

    let ready_before: Vec<TaskId> = if update.status == Some(TaskStatus::Completed) {
        store.tasks().filter(|t| store.is_ready(t)).map(|t| t.id.clone()).collect()
    } else {
        vec![]
    };

    let task = store.update(&id, update)?;

    let newly_ready: Vec<TaskSummary> = if task.status == TaskStatus::Completed {
        store.tasks().filter(|t| store.is_ready(t) && !ready_before.contains(&t.id)).map(TaskSummary::from).collect()
    } else {
        vec![]
    };

    Ok(TaskUpdateOutput { task: TaskSummary::from(&task), changes, newly_ready })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestTaskStore;

    fn update_input(id: &str, update: TaskUpdate) -> TaskUpdateInput {
        TaskUpdateInput { id: id.to_string(), update }
    }

    #[test]
    fn test_update_title() {
        let mut tasks = TestTaskStore::new();
        let task = tasks.root_task("Original");

        let input = update_input(
            task.id.as_str(),
            TaskUpdate { title: Some("Updated title".to_string()), ..Default::default() },
        );

        let output = execute_task_update(input, tasks.store_mut()).unwrap();

        assert_eq!(output.task.title, "Updated title");
        assert_eq!(output.changes, vec!["title"]);
    }

    #[test]
    fn test_update_status() {
        let mut tasks = TestTaskStore::new();
        let task = tasks.root_task("Task");

        let input =
            update_input(task.id.as_str(), TaskUpdate { status: Some(TaskStatus::InProgress), ..Default::default() });

        let output = execute_task_update(input, tasks.store_mut()).unwrap();

        assert_eq!(output.task.status, TaskStatus::InProgress);
        assert_eq!(output.changes, vec!["status"]);
    }

    #[test]
    fn test_update_multiple_fields() {
        let mut tasks = TestTaskStore::new();
        let task = tasks.root_task("Task");

        let input = update_input(
            task.id.as_str(),
            TaskUpdate {
                title: Some("New title".to_string()),
                description: Some("New description".to_string()),
                status: Some(TaskStatus::InProgress),
                assignee: Some("worker-1".to_string()),
                ..Default::default()
            },
        );

        let output = execute_task_update(input, tasks.store_mut()).unwrap();

        assert_eq!(output.changes.len(), 4);
        assert!(output.changes.contains(&"title".to_string()));
        assert!(output.changes.contains(&"description".to_string()));
        assert!(output.changes.contains(&"status".to_string()));
        assert!(output.changes.contains(&"assignee".to_string()));
    }

    #[test]
    fn test_update_nonexistent_task() {
        let mut tasks = TestTaskStore::new();

        let input =
            update_input("at-nonexistent", TaskUpdate { title: Some("New title".to_string()), ..Default::default() });

        let result = execute_task_update(input, tasks.store_mut());
        assert!(result.is_err());
    }

    #[test]
    fn test_update_with_invalid_deps() {
        let mut tasks = TestTaskStore::new();
        let task = tasks.root_task("Task");

        let input = update_input(
            task.id.as_str(),
            TaskUpdate { deps: Some(vec![TaskId::from("at-nonexistent")]), ..Default::default() },
        );

        let result = execute_task_update(input, tasks.store_mut());
        assert!(result.is_err());
    }

    #[test]
    fn test_complete_task() {
        let mut tasks = TestTaskStore::new();
        let task = tasks.root_task("Task to complete");

        let input = update_input(
            task.id.as_str(),
            TaskUpdate {
                status: Some(TaskStatus::Completed),
                summary: Some("Task done successfully".to_string()),
                ..Default::default()
            },
        );

        let output = execute_task_update(input, tasks.store_mut()).unwrap();

        assert_eq!(output.task.status, TaskStatus::Completed);
    }

    #[test]
    fn test_complete_unblocks_dependent() {
        let mut tasks = TestTaskStore::new();

        let root = tasks.root_task("Root");
        let sub1 = tasks.subtask(&root.id, "Subtask 1");
        let sub2 = tasks.subtask(&root.id, "Subtask 2");

        tasks
            .store_mut()
            .update(&sub2.id, TaskUpdate { deps: Some(vec![sub1.id.clone()]), ..Default::default() })
            .unwrap();

        let input = update_input(
            sub1.id.as_str(),
            TaskUpdate {
                status: Some(TaskStatus::Completed),
                summary: Some("Subtask 1 done".to_string()),
                ..Default::default()
            },
        );

        let output = execute_task_update(input, tasks.store_mut()).unwrap();

        assert_eq!(output.newly_ready.len(), 1);
        assert_eq!(output.newly_ready[0].id, sub2.id.to_string());
    }

    #[test]
    fn test_complete_with_flat_fields() {
        let mut tasks = TestTaskStore::new();
        let task = tasks.root_task("Research task");

        let input = update_input(
            task.id.as_str(),
            TaskUpdate {
                status: Some(TaskStatus::Completed),
                summary: Some("Found 3 issues".to_string()),
                decisions: Some(vec!["Use approach A".to_string()]),
                facts: Some(vec!["Issue in file X".to_string()]),
                next_steps: Some(vec!["Fix the issues".to_string()]),
                ..Default::default()
            },
        );

        let output = execute_task_update(input, tasks.store_mut()).unwrap();

        assert_eq!(output.task.status, TaskStatus::Completed);
        assert!(output.changes.contains(&"summary".to_string()));
        assert!(output.changes.contains(&"decisions".to_string()));
        assert!(output.changes.contains(&"facts".to_string()));
    }
}
