use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use acp_utils::notifications::ContextUsage;

/// Context lifecycle events.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContextEvent {
    /// Context compaction has been triggered.
    CompactionStarted { message_count: usize },
    /// Context was compacted to reduce token usage.
    CompactionResult { summary: String, messages_removed: usize },
    /// Context usage update for UI display.
    UsageUpdated { usage: ContextUsage },
    /// The agent context was cleared and reset to its blank state.
    Cleared,
}
