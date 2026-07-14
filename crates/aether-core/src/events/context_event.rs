use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use acp_utils::notifications::ContextUsage;

/// Terminal result of a context compaction operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CompactionOutcome {
    Completed,
    Failed { error: String },
    Cancelled,
}

/// Context lifecycle events.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContextEvent {
    /// Context compaction has been triggered.
    CompactionStarted { message_count: usize },
    /// Context compaction reached a terminal state.
    CompactionEnded { outcome: CompactionOutcome },
    /// Context was compacted to reduce token usage.
    CompactionResult { summary: String, messages_removed: usize },
    /// Context usage update for UI display.
    UsageUpdated { usage: ContextUsage },
    /// The agent context was cleared and reset to its blank state.
    Cleared,
}
