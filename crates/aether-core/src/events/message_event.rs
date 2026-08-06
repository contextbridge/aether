use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Whether a streamed text or thought chunk is the final one for its message.
///
/// `Partial` chunks stream as they arrive; a single `Complete` event carries the
/// full accumulated text and is emitted when the turn wraps up (which may be after
/// the originating LLM call's
/// [`TurnEvent::LlmCallEnded`](crate::events::TurnEvent::LlmCallEnded)).
///
/// This stands in for the raw `is_complete: bool` on event constructors so that
/// call sites read as `StreamState::Complete` instead of an opaque `true` literal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamState {
    /// More chunks may follow for this message.
    Partial,
    /// This is the final chunk for the message.
    Complete,
}

impl StreamState {
    pub fn is_complete(self) -> bool {
        matches!(self, Self::Complete)
    }
}

/// Streaming message content from the agent.
///
/// Chunks stream with `is_complete: false`; a final event with `is_complete: true`
/// carries the full accumulated text. The completion event is emitted when the
/// turn wraps up, which may be after the originating LLM call's
/// [`TurnEvent::LlmCallEnded`](crate::events::TurnEvent::LlmCallEnded).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessageEvent {
    /// Assistant response text.
    Text { message_id: String, chunk: String, is_complete: bool },
    /// Assistant reasoning summary text.
    Thought { message_id: String, chunk: String, is_complete: bool },
}
