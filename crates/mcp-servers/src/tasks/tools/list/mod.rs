use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::common::TaskSummary;
use crate::tasks::task_store::TaskStore;
use crate::tasks::types::{TaskId, TaskStatus};
use utils::display_meta::{ToolDisplayMeta, ToolResultMeta};

/// Input for the `task_list` tool
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TaskListInput {
    /// Filter by assignee
    #[serde(default)]
    pub assignee: Option<String>,

    /// Filter by status: pending, `in_progress`, completed, or blocked
    #[serde(default)]
    pub status: Option<TaskStatus>,

    /// List all tasks in a specific tree (by root task ID)
    #[serde(default, alias = "tree_id")]
    pub tree_id: Option<String>,

    /// Only return tasks that are ready to start (pending with all deps completed)
    #[serde(default, alias = "ready_only")]
    pub ready_only: Option<bool>,
}

/// Output for the `task_list` tool
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TaskListOutput {
    /// Matching tasks
    pub tasks: Vec<TaskSummary>,

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
        store.list_by_status(status).into_iter().map(TaskSummary::from).collect()
    } else {
        store
            .list_trees()
            .iter()
            .flat_map(|root_id| store.get_tree(root_id).unwrap_or_default().into_iter().map(TaskSummary::from))
            .collect()
    };

    let done = tasks.iter().filter(|t| t.status == TaskStatus::Completed).count();
    let display_meta = ToolDisplayMeta::new("Todo", format!("{done}/{} tasks", tasks.len()));

    TaskListOutput { tasks, meta: Some(display_meta.into()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::TaskUpdate;
    use crate::testing::TestTaskStore;

    #[test]
    fn test_list_all_tasks() {
        let mut tasks = TestTaskStore::new();

        let root = tasks.root_task("Root");
        tasks.subtask(&root.id, "Subtask 1");
        tasks.subtask(&root.id, "Subtask 2");

        let output = execute_task_list(&TaskListInput::default(), tasks.store());

        assert_eq!(output.tasks.len(), 3);
    }

    #[test]
    fn test_list_by_tree() {
        let mut tasks = TestTaskStore::new();

        let root1 = tasks.root_task("Root 1");
        tasks.subtask(&root1.id, "Subtask 1.1");

        let root2 = tasks.root_task("Root 2");
        tasks.subtask(&root2.id, "Subtask 2.1");
        tasks.subtask(&root2.id, "Subtask 2.2");

        let output = execute_task_list(
            &TaskListInput { tree_id: Some(root2.id.to_string()), ..Default::default() },
            tasks.store(),
        );

        assert_eq!(output.tasks.len(), 3);
    }

    #[test]
    fn test_list_by_assignee() {
        let mut tasks = TestTaskStore::new();

        let root = tasks.root_task("Root");
        let sub1 = tasks.subtask(&root.id, "Task 1");
        let sub2 = tasks.subtask(&root.id, "Task 2");

        tasks
            .store_mut()
            .update(&sub1.id, TaskUpdate { assignee: Some("worker-1".to_string()), ..Default::default() })
            .unwrap();
        tasks
            .store_mut()
            .update(&sub2.id, TaskUpdate { assignee: Some("worker-2".to_string()), ..Default::default() })
            .unwrap();

        let output = execute_task_list(
            &TaskListInput { assignee: Some("worker-1".to_string()), ..Default::default() },
            tasks.store(),
        );

        assert_eq!(output.tasks.len(), 1);
        assert_eq!(output.tasks[0].assignee, Some("worker-1".to_string()));
    }

    #[test]
    fn test_list_by_status() {
        let mut tasks = TestTaskStore::new();

        let root = tasks.root_task("Root");
        let sub = tasks.subtask(&root.id, "Subtask");

        tasks
            .store_mut()
            .update(&sub.id, TaskUpdate { status: Some(TaskStatus::InProgress), ..Default::default() })
            .unwrap();

        let output = execute_task_list(
            &TaskListInput { status: Some(TaskStatus::InProgress), ..Default::default() },
            tasks.store(),
        );

        assert_eq!(output.tasks.len(), 1);
        assert_eq!(output.tasks[0].status, TaskStatus::InProgress);
    }

    #[test]
    fn test_list_ready_only() {
        let mut tasks = TestTaskStore::new();

        let root = tasks.root_task("Root");
        let sub1 = tasks.subtask(&root.id, "Subtask 1");
        let sub2 = tasks.subtask(&root.id, "Subtask 2");

        tasks
            .store_mut()
            .update(&sub2.id, TaskUpdate { deps: Some(vec![sub1.id.clone()]), ..Default::default() })
            .unwrap();

        let output = execute_task_list(&TaskListInput { ready_only: Some(true), ..Default::default() }, tasks.store());

        assert_eq!(output.tasks.len(), 2);

        tasks
            .store_mut()
            .update(
                &sub1.id,
                TaskUpdate {
                    status: Some(TaskStatus::Completed),
                    summary: Some("done".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();

        let output = execute_task_list(&TaskListInput { ready_only: Some(true), ..Default::default() }, tasks.store());

        assert_eq!(output.tasks.len(), 2);
        let ready_ids: Vec<_> = output.tasks.iter().map(|t| t.id.as_str()).collect();
        assert!(ready_ids.contains(&sub2.id.as_str()));
    }

    #[test]
    fn test_list_empty() {
        let tasks = TestTaskStore::new();

        let output = execute_task_list(&TaskListInput::default(), tasks.store());

        assert_eq!(output.tasks.len(), 0);
    }

    #[test]
    fn test_list_nonexistent_tree() {
        let tasks = TestTaskStore::new();

        let output = execute_task_list(
            &TaskListInput { tree_id: Some("at-nonexistent".to_string()), ..Default::default() },
            tasks.store(),
        );

        assert_eq!(output.tasks.len(), 0);
    }
}
