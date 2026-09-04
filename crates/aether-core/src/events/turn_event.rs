use llm::{ContentBlock, LlmCallPurpose, LlmError, ModelIdentity, StopReason, TokenUsage};
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
    Completed {
        stop_reason: Option<StopReason>,
        usage: Option<TokenUsage>,
    },
    Failed {
        error: String,
        will_retry: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        http_status: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_request_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_error_code: Option<String>,
    },
    Cancelled,
}

impl LlmCallOutcome {
    pub fn failed(error: impl Into<String>, will_retry: bool) -> Self {
        Self::Failed {
            error: error.into(),
            will_retry,
            http_status: None,
            provider_request_id: None,
            provider_error_code: None,
        }
    }

    pub fn from_llm_error(error: &LlmError, will_retry: bool) -> Self {
        let Some(provider) = error.provider() else {
            return Self::failed(error.to_string(), will_retry);
        };
        Self::Failed {
            error: provider.to_string(),
            will_retry,
            http_status: provider.http_status,
            provider_request_id: provider.request_id.clone(),
            provider_error_code: provider.code.clone(),
        }
    }
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
    Started {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        content: Vec<ContentBlock>,
    },
    /// A retry is waiting for its backoff delay before the request starts.
    RetryScheduled { purpose: LlmCallPurpose, attempt: u32, max_attempts: u32, delay_ms: u64 },
    /// An LLM request was issued.
    LlmCallStarted {
        purpose: LlmCallPurpose,
        model: ModelIdentity,
        display_name: String,
        /// 0 for the initial call, incrementing per retry.
        attempt: u32,
        max_attempts: u32,
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
            Self::RetryScheduled { attempt, max_attempts, delay_ms, .. } => {
                Some(RetryInfo { attempt: *attempt, max_attempts: *max_attempts, delay_ms: *delay_ms })
            }
            _ => None,
        }
    }
}
