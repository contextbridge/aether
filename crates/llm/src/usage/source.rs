use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The agent a usage sample belongs to and where it sits in the sub-agent
/// tree. A freshly created source has no parent; the agent that receives a
/// sub-agent's usage fills in `parent_agent_id` and `task_id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UsageSource {
    pub agent_id: String,
    pub parent_agent_id: Option<String>,
    pub task_id: Option<String>,
    pub agent_name: String,
}

impl UsageSource {
    pub fn new(agent_name: impl Into<String>) -> Self {
        Self {
            agent_id: Uuid::new_v4().to_string(),
            parent_agent_id: None,
            task_id: None,
            agent_name: agent_name.into(),
        }
    }
}
