use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

use super::common::{TaskSummary, task_result_meta};
use crate::tasks::task_store::{TaskStore, TaskStoreError};
use crate::tasks::types::{TaskId, TaskUpdate};
use utils::display_meta::ToolResultMeta;

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
    /// The created task
    pub task: TaskSummary,

    /// Display metadata for human-friendly rendering
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub meta: Option<ToolResultMeta>,
}

/// Create a new task or subtask
pub fn execute_task_create(input: &TaskCreateInput, store: &mut TaskStore) -> Result<TaskCreateOutput, TaskStoreError> {
    let fields = TaskUpdate {
        description: input.description.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned),
        assignee: input.assignee.clone(),
        deps: input.deps.as_ref().map(|deps| deps.iter().map(|id| TaskId::from(id.as_str())).collect()),
        ..Default::default()
    };
    let parent = input.parent_id.as_deref().map(TaskId::from);
    let task = store.create(parent.as_ref(), &input.title, fields)?;

    Ok(TaskCreateOutput { task: TaskSummary::from(&task), meta: Some(task_result_meta(store, &task.title)) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestTaskStore;
    use serde_json::json;

    #[test]
    fn test_create_root_task() {
        let mut tasks = TestTaskStore::new();

        let input = TaskCreateInput {
            title: "Research AI agents".to_string(),
            description: Some("Investigate multi-agent patterns".to_string()),
            parent_id: None,
            assignee: Some("orchestrator".to_string()),
            deps: None,
        };

        let output = execute_task_create(&input, tasks.store_mut()).unwrap();

        assert_eq!(output.task.title, "Research AI agents");
        assert_eq!(output.task.assignee, Some("orchestrator".to_string()));
        assert_eq!(output.task.parent, None);
    }

    #[test]
    fn test_create_subtask() {
        let mut tasks = TestTaskStore::new();

        let root_input = TaskCreateInput {
            title: "Root task".to_string(),
            description: Some("Root task description".to_string()),
            parent_id: None,
            assignee: None,
            deps: None,
        };
        let root = execute_task_create(&root_input, tasks.store_mut()).unwrap();

        let sub_input = TaskCreateInput {
            title: "Subtask 1".to_string(),
            description: Some("Do something specific".to_string()),
            parent_id: Some(root.task.id.clone()),
            assignee: Some("worker-1".to_string()),
            deps: None,
        };

        let output = execute_task_create(&sub_input, tasks.store_mut()).unwrap();

        assert_eq!(output.task.title, "Subtask 1");
        assert_eq!(output.task.parent, Some(root.task.id));
    }

    #[test]
    fn test_create_with_deps() {
        let mut tasks = TestTaskStore::new();

        let root = tasks.root_task("Root");
        let sub1 = tasks.subtask(&root.id, "Subtask 1");

        let input = TaskCreateInput {
            title: "Subtask 2".to_string(),
            description: Some("Subtask with dependency".to_string()),
            parent_id: Some(root.id.to_string()),
            assignee: None,
            deps: Some(vec![sub1.id.to_string()]),
        };

        let output = execute_task_create(&input, tasks.store_mut()).unwrap();

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
        let mut tasks = TestTaskStore::new();

        let input: TaskCreateInput = serde_json::from_value(json!({
            "title": "Root without description"
        }))
        .unwrap();

        let output = execute_task_create(&input, tasks.store_mut()).unwrap();
        let created = tasks.store().get(&TaskId::from(output.task.id.as_str())).unwrap();

        assert_eq!(output.task.title, "Root without description");
        assert_eq!(output.task.parent, None);
        assert_eq!(created.description, None);
    }

    #[test]
    fn test_description_can_be_omitted_for_subtask() {
        let mut tasks = TestTaskStore::new();
        let root = tasks.root_task("Root");

        let input: TaskCreateInput = serde_json::from_value(json!({
            "title": "Subtask without description",
            "parent_id": root.id
        }))
        .unwrap();

        let output = execute_task_create(&input, tasks.store_mut()).unwrap();
        let created = tasks.store().get(&TaskId::from(output.task.id.as_str())).unwrap();

        assert_eq!(output.task.title, "Subtask without description");
        assert_eq!(output.task.parent, Some(root.id.to_string()));
        assert_eq!(created.description, None);
    }

    #[test]
    fn test_create_with_empty_parent_id_treated_as_root() {
        let mut tasks = TestTaskStore::new();

        let input: TaskCreateInput = serde_json::from_value(json!({
            "title": "Root from empty parent_id",
            "description": "Created as root",
            "parent_id": ""
        }))
        .unwrap();

        let output = execute_task_create(&input, tasks.store_mut()).unwrap();

        assert_eq!(output.task.parent, None);
    }

    #[test]
    fn test_create_with_whitespace_parent_id_treated_as_root() {
        let mut tasks = TestTaskStore::new();

        let input: TaskCreateInput = serde_json::from_value(json!({
            "title": "Root from whitespace parent_id",
            "description": "Created as root",
            "parent_id": "  \t\n  "
        }))
        .unwrap();

        let output = execute_task_create(&input, tasks.store_mut()).unwrap();

        assert_eq!(output.task.parent, None);
    }

    #[test]
    fn test_create_with_invalid_parent() {
        let mut tasks = TestTaskStore::new();

        let input = TaskCreateInput {
            title: "Orphan".to_string(),
            description: Some("Orphan task".to_string()),
            parent_id: Some("at-nonexistent".to_string()),
            assignee: None,
            deps: None,
        };

        let result = execute_task_create(&input, tasks.store_mut());
        assert!(result.is_err());
    }

    #[test]
    fn invalid_deps_create_nothing() {
        let mut tasks = TestTaskStore::new();
        let root = tasks.root_task("Root");

        for parent_id in [None, Some(root.id.to_string())] {
            let input = TaskCreateInput {
                title: "Blocked".to_string(),
                description: None,
                parent_id,
                assignee: None,
                deps: Some(vec!["at-missing".to_string()]),
            };

            let result = execute_task_create(&input, tasks.store_mut());

            assert!(matches!(result, Err(TaskStoreError::DependencyNotFound { .. })), "{result:?}");
        }
        tasks.reopen();
        assert_eq!(tasks.store().len(), 1);
    }

    #[test]
    fn subtask_of_a_missing_subtask_is_rejected() {
        let mut tasks = TestTaskStore::new();
        let root = tasks.root_task("Root");

        let input = TaskCreateInput {
            title: "Orphan".to_string(),
            description: None,
            parent_id: Some(format!("{}.99", root.id)),
            assignee: None,
            deps: None,
        };

        let result = execute_task_create(&input, tasks.store_mut());

        assert!(matches!(result, Err(TaskStoreError::ParentNotFound { .. })), "{result:?}");
        tasks.reopen();
        assert_eq!(tasks.store().len(), 1);
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
}
