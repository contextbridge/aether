use llm::{StopReason, TokenUsage};
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

/// Turn lifecycle events.
///
/// A turn spans from a user message to a terminal [`TurnEvent::Ended`]. Within a
/// turn, each LLM call is bracketed by `LlmCallStarted`/`LlmCallEnded`; retries
/// surface as an `LlmCallStarted` with `attempt > 0`. Note that the completion
/// events for streamed message content
/// ([`MessageEvent`](crate::events::MessageEvent) with `is_complete: true`) are
/// emitted at turn completion, after the originating call's `LlmCallEnded`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TurnEvent {
    /// A user message began a turn. Messages queued while a turn is active are
    /// folded into that turn and do not start a new one.
    Started,
    /// An LLM call was issued.
    LlmCallStarted {
        purpose: LlmCallPurpose,
        provider: Option<String>,
        model: Option<String>,
        display_name: String,
        /// 0 for the initial call, incrementing per retry.
        attempt: u32,
        max_attempts: u32,
        /// Backoff delay before a retry fires; `None` on the initial call.
        delay_ms: Option<u64>,
    },
    /// An LLM call reached a terminal state.
    LlmCallEnded { purpose: LlmCallPurpose, outcome: LlmCallOutcome },
    /// The agent is auto-continuing because the LLM stopped with a resumable
    /// stop reason.
    AutoContinue { attempt: u32, max_attempts: u32 },
    /// The turn reached a terminal state.
    Ended { outcome: TurnOutcome },
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
