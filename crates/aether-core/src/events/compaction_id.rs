use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Protocol-independent identity persisted across a compaction's lifecycle.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct CompactionId(String);

impl CompactionId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for CompactionId {
    fn default() -> Self {
        Self::new()
    }
}

impl From<String> for CompactionId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for CompactionId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}
