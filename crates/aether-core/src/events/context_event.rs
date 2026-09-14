use super::CompactionId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use llm::{ContextUsage, MessageId};

/// Terminal result of a context compaction operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CompactionOutcome {
    Completed,
    Failed { error: String },
    Cancelled,
}

/// Context lifecycle events.
///
/// Compaction IDs are required in persisted and headless events. Logs without
/// these IDs are not migrated or assigned identities during deserialization.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContextEvent {
    /// Context compaction has been triggered.
    CompactionStarted { compaction_id: CompactionId, message_count: usize },
    /// Context compaction reached a terminal state.
    CompactionEnded { compaction_id: CompactionId, outcome: CompactionOutcome },
    /// Context was compacted to reduce token usage.
    CompactionResult { compaction_id: CompactionId, message_id: MessageId, summary: String, messages_removed: usize },
    /// Context usage update for UI display.
    UsageUpdated { usage: ContextUsage },
    /// The agent context was cleared and reset to its blank state.
    Cleared,
}
