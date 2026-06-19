use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::tasks::task_store::{TaskStore, TaskStoreError};
use crate::tasks::types::{Task, TaskId};
use mcp_utils::display_meta::{ToolDisplayMeta, ToolResultMeta};

/// Input for the `task_get` tool
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TaskGetInput {
    /// Task ID to retrieve (e.g., "at-a1b2c3d4" or "at-a1b2c3d4.1")
    pub id: String,
}

/// Output for the `task_get` tool
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TaskGetOutput {
    pub status: String,
    pub task: Task,
    pub message: String,
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub meta: Option<ToolResultMeta>,
}

/// Get a task by ID
pub fn execute_task_get(input: TaskGetInput, store: &TaskStore) -> Result<TaskGetOutput, TaskStoreError> {
    let task_id = TaskId::from(input.id.as_str());

    let task = store.get(&task_id).ok_or(TaskStoreError::NotFound { id: input.id })?;

    let display_meta = ToolDisplayMeta::new("Todo", task.title.clone());

    Ok(TaskGetOutput {
        status: "success".to_string(),
        message: format!("Retrieved task '{}'", task.title),
        task: task.clone(),
        meta: Some(display_meta.into()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::types::{TaskStatus, TaskUpdate};
    use tempfile::TempDir;

    fn setup() -> (TempDir, TaskStore) {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path().join(".aether-tasks");
        let mut store = TaskStore::new(root);
        store.init().unwrap();
        (temp_dir, store)
    }

    #[test]
    fn test_get_root_task() {
        let (_temp, mut store) = setup();

        let created = store.create_tree("Research topic", Some("Detailed description")).unwrap();

        let output = execute_task_get(TaskGetInput { id: created.id.to_string() }, &store).unwrap();

        assert_eq!(output.status, "success");
        assert_eq!(output.task.id.to_string(), created.id.to_string());
        assert_eq!(output.task.title, "Research topic");
        assert_eq!(output.task.description, Some("Detailed description".to_string()));
        assert_eq!(output.task.status, TaskStatus::Pending);
        assert!(output.message.contains("Retrieved task"));
    }

    #[test]
    fn test_get_subtask() {
        let (_temp, mut store) = setup();

        let root = store.create_tree("Root", None).unwrap();
        let subtask = store.add_subtask(&root.id, "Subtask 1").unwrap();

        let output = execute_task_get(TaskGetInput { id: subtask.id.to_string() }, &store).unwrap();

        assert_eq!(output.task.id.to_string(), subtask.id.to_string());
        assert_eq!(output.task.title, "Subtask 1");
        assert_eq!(output.task.parent, Some(root.id.clone()));
    }

    #[test]
    fn test_get_task_with_flat_fields() {
        let (_temp, mut store) = setup();

        let task = store.create_tree("Task to complete", None).unwrap();

        store
            .update(
                &task.id,
                TaskUpdate {
                    status: Some(TaskStatus::Completed),
                    summary: Some("Found the answer".to_string()),
                    decisions: Some(vec!["Chose option A".to_string()]),
                    facts: Some(vec!["Discovered X".to_string()]),
                    next_steps: Some(vec!["Do Y next".to_string()]),
                    files_read: Some(vec!["src/main.rs".to_string()]),
                    ..Default::default()
                },
            )
            .unwrap();

        let output = execute_task_get(TaskGetInput { id: task.id.to_string() }, &store).unwrap();

        assert_eq!(output.task.status, TaskStatus::Completed);
        assert_eq!(output.task.summary, Some("Found the answer".to_string()));
        assert_eq!(output.task.decisions, vec!["Chose option A"]);
        assert_eq!(output.task.facts, vec!["Discovered X"]);
        assert_eq!(output.task.next_steps, vec!["Do Y next"]);
        assert_eq!(output.task.files_read, vec!["src/main.rs"]);
    }

    #[test]
    fn test_get_task_with_deps() {
        let (_temp, mut store) = setup();

        let root = store.create_tree("Root", None).unwrap();
        let sub1 = store.add_subtask(&root.id, "Subtask 1").unwrap();
        let sub2 = store.add_subtask(&root.id, "Subtask 2").unwrap();

        store.update(&sub2.id, TaskUpdate { deps: Some(vec![sub1.id.clone()]), ..Default::default() }).unwrap();

        let output = execute_task_get(TaskGetInput { id: sub2.id.to_string() }, &store).unwrap();

        assert_eq!(output.task.deps, vec![sub1.id.clone()]);
    }

    #[test]
    fn test_get_nonexistent_task() {
        let (_temp, store) = setup();

        let result = execute_task_get(TaskGetInput { id: "at-nonexistent".to_string() }, &store);

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), TaskStoreError::NotFound { .. }));
    }
}
