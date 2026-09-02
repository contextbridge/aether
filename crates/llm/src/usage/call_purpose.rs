use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What an LLM call was issued for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LlmCallPurpose {
    Chat,
    Compaction,
}

impl LlmCallPurpose {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Compaction => "compaction",
        }
    }
}
