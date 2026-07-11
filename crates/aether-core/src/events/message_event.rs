use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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
