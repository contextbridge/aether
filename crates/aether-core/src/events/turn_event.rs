use llm::{ContentBlock, StopReason, TokenUsage};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// How a turn reached its terminal state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TurnOutcome {
    Completed,
    Cancelled,
    Failed { error: String },
}

/// How a single LLM call ended.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum LlmCallOutcome {
    Completed { stop_reason: Option<StopReason>, usage: Option<TokenUsage> },
    Failed { error: String, will_retry: bool },
    Cancelled,
}

/// What an LLM call was issued for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LlmCallPurpose {
    Chat,
    Compaction,
}

/// A retry of a failed LLM call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryInfo {
    pub attempt: u32,
    pub max_attempts: u32,
    pub delay_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TurnEvent {
    Started {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        content: Vec<ContentBlock>,
    },
    LlmCallStarted {
        purpose: LlmCallPurpose,
        provider: Option<String>,
        model: Option<String>,
        display_name: String,
        attempt: u32,
        max_attempts: u32,
        delay_ms: Option<u64>,
    },
    LlmCallEnded {
        purpose: LlmCallPurpose,
        outcome: LlmCallOutcome,
    },
    AutoContinue {
        attempt: u32,
        max_attempts: u32,
    },
    Ended {
        outcome: TurnOutcome,
    },
}

impl TurnEvent {
    pub fn retry_info(&self) -> Option<RetryInfo> {
        match self {
            Self::LlmCallStarted { attempt, max_attempts, delay_ms: Some(delay_ms), .. } if *attempt > 0 => {
                Some(RetryInfo { attempt: *attempt, max_attempts: *max_attempts, delay_ms: *delay_ms })
            }
            _ => None,
        }
    }
}
