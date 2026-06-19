use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

use super::common::{TaskSummary, task_result_meta};
use crate::tasks::task_store::{TaskStore, TaskStoreError};
use crate::tasks::types::{TaskId, TaskUpdate};
use mcp_utils::display_meta::ToolResultMeta;

fn empty_string_as_none<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<String>::deserialize(deserializer)?;
    Ok(value.and_then(|s| {
        let trimmed = s.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    }))
}

/// Input for the `task_create` tool
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TaskCreateInput {
    /// Short descriptive title for the task
    pub title: String,

    /// Optional detailed description (markdown)
    #[serde(default)]
    pub description: Option<String>,

    /// Parent task ID - if provided, creates a subtask
    #[serde(default, deserialize_with = "empty_string_as_none", alias = "parent_id")]
    pub parent_id: Option<String>,

    /// Agent or worker to assign the task to
    #[serde(default)]
    pub assignee: Option<String>,

    /// Task IDs that must complete before this task can start
    #[serde(default)]
    pub deps: Option<Vec<String>>,
}

/// Output for the `task_create` tool
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TaskCreateOutput {
    /// Status of the operation
    pub status: String,

    /// The created task
    pub task: TaskSummary,

    /// Human-readable message
    pub message: String,

    /// Display metadata for human-friendly rendering
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub meta: Option<ToolResultMeta>,
}

/// Create a new task or subtask
pub fn execute_task_create(input: &TaskCreateInput, store: &mut TaskStore) -> Result<TaskCreateOutput, TaskStoreError> {
    let normalized_parent_id = input.parent_id.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let description = input.description.as_deref().map(str::trim).filter(|s| !s.is_empty());

    let task = if let Some(parent_id) = normalized_parent_id {
        let parent = TaskId::from(parent_id);
        store.add_subtask(&parent, &input.title)?
    } else {
        store.create_tree(&input.title, description)?
    };

    let is_subtask = normalized_parent_id.is_some();
    let needs_update = input.assignee.is_some() || input.deps.is_some() || (is_subtask && description.is_some());

    let task = if needs_update {
        let update = TaskUpdate {
            description: if is_subtask {
                description.map(str::to_owned)
            } else {
                None // Already set during create_tree
            },
            assignee: input.assignee.clone(),
            deps: input.deps.as_ref().map(|d| d.iter().map(|s| TaskId::from(s.as_str())).collect()),
            ..Default::default()
        };
        store.update(&task.id, update)?
    } else {
        task
    };

    let task_type = if is_subtask { "subtask" } else { "task tree" };

    Ok(TaskCreateOutput {
        status: "success".to_string(),
        message: format!("Created {} '{}' with ID {}", task_type, task.title, task.id),
        task: TaskSummary::from(&task),
        meta: Some(task_result_meta(store, &task.title)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    fn setup() -> (TempDir, TaskStore) {
        let temp_dir = TempDir::new().unwrap();
        let root = temp_dir.path().join(".aether-tasks");
        let mut store = TaskStore::new(root);
        store.init().unwrap();
        (temp_dir, store)
    }

    #[test]
    fn test_create_root_task() {
        let (_temp, mut store) = setup();

        let input = TaskCreateInput {
            title: "Research AI agents".to_string(),
            description: Some("Investigate multi-agent patterns".to_string()),
            parent_id: None,
            assignee: Some("orchestrator".to_string()),
            deps: None,
        };

        let output = execute_task_create(&input, &mut store).unwrap();

        assert_eq!(output.status, "success");
        assert_eq!(output.task.title, "Research AI agents");
        assert_eq!(output.task.assignee, Some("orchestrator".to_string()));
        assert!(output.message.contains("task tree"));
    }

    #[test]
    fn test_create_subtask() {
        let (_temp, mut store) = setup();

        let root_input = TaskCreateInput {
            title: "Root task".to_string(),
            description: Some("Root task description".to_string()),
            parent_id: None,
            assignee: None,
            deps: None,
        };
        let root = execute_task_create(&root_input, &mut store).unwrap();

        let sub_input = TaskCreateInput {
            title: "Subtask 1".to_string(),
            description: Some("Do something specific".to_string()),
            parent_id: Some(root.task.id.clone()),
            assignee: Some("worker-1".to_string()),
            deps: None,
        };

        let output = execute_task_create(&sub_input, &mut store).unwrap();

        assert_eq!(output.status, "success");
        assert_eq!(output.task.title, "Subtask 1");
        assert_eq!(output.task.parent, Some(root.task.id));
        assert!(output.message.contains("subtask"));
    }

    #[test]
    fn test_create_with_deps() {
        let (_temp, mut store) = setup();

        let root = store.create_tree("Root", None).unwrap();
        let sub1 = store.add_subtask(&root.id, "Subtask 1").unwrap();

        let input = TaskCreateInput {
            title: "Subtask 2".to_string(),
            description: Some("Subtask with dependency".to_string()),
            parent_id: Some(root.id.to_string()),
            assignee: None,
            deps: Some(vec![sub1.id.to_string()]),
        };

        let output = execute_task_create(&input, &mut store).unwrap();

        assert_eq!(output.task.deps, vec![sub1.id.to_string()]);
    }

    #[test]
    fn test_parent_id_empty_string_deserializes_to_none() {
        let input: TaskCreateInput = serde_json::from_value(json!({
            "title": "Root via empty parent",
            "description": "Should be root",
            "parent_id": ""
        }))
        .unwrap();

        assert_eq!(input.parent_id, None);
    }

    #[test]
    fn test_parent_id_whitespace_deserializes_to_none() {
        let input: TaskCreateInput = serde_json::from_value(json!({
            "title": "Root via whitespace parent",
            "description": "Should be root",
            "parent_id": "   "
        }))
        .unwrap();

        assert_eq!(input.parent_id, None);
    }

    #[test]
    fn test_description_can_be_omitted_for_root_task() {
        let (_temp, mut store) = setup();

        let input: TaskCreateInput = serde_json::from_value(json!({
            "title": "Root without description"
        }))
        .unwrap();

        let output = execute_task_create(&input, &mut store).unwrap();
        let created = store.get(&TaskId::from(output.task.id.as_str())).unwrap();

        assert_eq!(output.task.title, "Root without description");
        assert_eq!(output.task.parent, None);
        assert_eq!(created.description, None);
    }

    #[test]
    fn test_description_can_be_omitted_for_subtask() {
        let (_temp, mut store) = setup();

        let root = store.create_tree("Root", None).unwrap();

        let input: TaskCreateInput = serde_json::from_value(json!({
            "title": "Subtask without description",
            "parent_id": root.id
        }))
        .unwrap();

        let output = execute_task_create(&input, &mut store).unwrap();
        let created = store.get(&TaskId::from(output.task.id.as_str())).unwrap();

        assert_eq!(output.task.title, "Subtask without description");
        assert_eq!(output.task.parent, Some(root.id.to_string()));
        assert_eq!(created.description, None);
    }

    #[test]
    fn test_create_with_empty_parent_id_treated_as_root() {
        let (_temp, mut store) = setup();

        let input: TaskCreateInput = serde_json::from_value(json!({
            "title": "Root from empty parent_id",
            "description": "Created as root",
            "parent_id": ""
        }))
        .unwrap();

        let output = execute_task_create(&input, &mut store).unwrap();

        assert_eq!(output.task.parent, None);
        assert!(output.message.contains("task tree"));
    }

    #[test]
    fn test_create_with_whitespace_parent_id_treated_as_root() {
        let (_temp, mut store) = setup();

        let input: TaskCreateInput = serde_json::from_value(json!({
            "title": "Root from whitespace parent_id",
            "description": "Created as root",
            "parent_id": "  \t\n  "
        }))
        .unwrap();

        let output = execute_task_create(&input, &mut store).unwrap();

        assert_eq!(output.task.parent, None);
        assert!(output.message.contains("task tree"));
    }

    #[test]
    fn test_create_with_invalid_parent() {
        let (_temp, mut store) = setup();

        let input = TaskCreateInput {
            title: "Orphan".to_string(),
            description: Some("Orphan task".to_string()),
            parent_id: Some("at-nonexistent".to_string()),
            assignee: None,
            deps: None,
        };

        let result = execute_task_create(&input, &mut store);
        assert!(result.is_err());
    }

    #[test]
    fn test_input_accepts_camel_case() {
        let input: TaskCreateInput = serde_json::from_value(json!({
            "title": "Task with camelCase",
            "description": "Using parentId",
            "parentId": "at-parent"
        }))
        .unwrap();

        assert_eq!(input.title, "Task with camelCase");
        assert_eq!(input.parent_id, Some("at-parent".to_string()));
    }

    #[test]
    fn test_input_accepts_snake_case_alias() {
        let input: TaskCreateInput = serde_json::from_value(json!({
            "title": "Task with snake_case",
            "description": "Using parent_id",
            "parent_id": "at-parent"
        }))
        .unwrap();

        assert_eq!(input.title, "Task with snake_case");
        assert_eq!(input.parent_id, Some("at-parent".to_string()));
    }

    #[test]
    fn test_output_uses_camel_case() {
        let (_temp, mut store) = setup();

        let input = TaskCreateInput {
            title: "Test camelCase output".to_string(),
            description: None,
            parent_id: None,
            assignee: None,
            deps: None,
        };

        let output = execute_task_create(&input, &mut store).unwrap();
        let json = serde_json::to_string(&output).unwrap();

        // Verify output uses camelCase field names
        assert!(json.contains("\"status\""));
        assert!(json.contains("\"task\""));
        assert!(json.contains("\"message\""));
    }
}
