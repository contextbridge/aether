use schemars::JsonSchema;
use serde::Serialize;

use crate::tasks::task_store::TaskStore;
use crate::tasks::types::{Task, TaskStatus};
use mcp_utils::display_meta::{PlanMeta, PlanMetaEntry, PlanMetaStatus, ToolDisplayMeta, ToolResultMeta};

impl From<TaskStatus> for PlanMetaStatus {
    fn from(status: TaskStatus) -> Self {
        match status {
            TaskStatus::InProgress => PlanMetaStatus::InProgress,
            TaskStatus::Completed => PlanMetaStatus::Completed,
            // Pending and Blocked both map to Pending in the plan view
            TaskStatus::Pending | TaskStatus::Blocked => PlanMetaStatus::Pending,
        }
    }
}

/// Summary of a task for tool output (compact form for list views)
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TaskSummary {
    pub id: String,
    pub title: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub deps: Vec<String>,
}

impl From<&Task> for TaskSummary {
    fn from(task: &Task) -> Self {
        Self {
            id: task.id.to_string(),
            title: task.title.clone(),
            status: task.status.as_wire_str().to_string(),
            parent: task.parent.as_ref().map(std::string::ToString::to_string),
            assignee: task.assignee.clone(),
            deps: task.deps.iter().map(std::string::ToString::to_string).collect(),
        }
    }
}

/// Build a [`ToolResultMeta`] with a plan snapshot from the task store.
pub fn task_result_meta(store: &TaskStore, title: &str) -> ToolResultMeta {
    ToolResultMeta::with_plan(ToolDisplayMeta::new("Todo", title), build_plan_meta(store))
}

/// Collect all active tasks into a [`PlanMeta`] snapshot.
fn build_plan_meta(store: &TaskStore) -> PlanMeta {
    let entries = store
        .list_trees()
        .iter()
        .flat_map(|root_id| store.get_tree(root_id).unwrap_or_default())
        .map(|task| PlanMetaEntry { content: task.title.clone(), status: task.status.into() })
        .collect();
    PlanMeta { entries }
}
