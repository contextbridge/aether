use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Context/token usage reported after an LLM call.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ContextUsage {
    pub usage_ratio: Option<f64>,
    pub context_limit: Option<u32>,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: Option<u32>,
    pub cache_creation_tokens: Option<u32>,
    pub reasoning_tokens: Option<u32>,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cache_read_tokens: u64,
    pub total_cache_creation_tokens: u64,
    pub total_reasoning_tokens: u64,
}

impl ContextUsage {
    pub fn total_tokens(&self) -> u64 {
        self.total_input_tokens + self.total_output_tokens
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContextEvent {
    CompactionStarted { message_count: usize },
    CompactionResult { summary: String, messages_removed: usize },
    UsageUpdated { usage: ContextUsage },
    Cleared,
}
