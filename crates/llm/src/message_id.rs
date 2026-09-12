use std::fmt::{Display, Formatter};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Provider-independent message id
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct MessageId(String);

impl MessageId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Stable identity of the separately displayed reasoning component.
    pub fn thought(&self) -> Self {
        Self(format!("{}:thought", self.0))
    }

    /// Namespaced identity derived from the persisted tool-call correlation ID.
    pub fn tool_result(tool_call_id: &str) -> Self {
        Self(format!("tool-result:{tool_call_id}"))
    }

    pub fn task_result(task_id: &str) -> Self {
        Self(format!("task-result:{task_id}"))
    }
}

impl Default for MessageId {
    fn default() -> Self {
        Self::new()
    }
}

impl From<String> for MessageId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for MessageId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl std::ops::Deref for MessageId {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl Display for MessageId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
