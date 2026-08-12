use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{ContextEvent, MessageEvent, ModelEvent, StreamState, ToolEvent, TurnEvent, TurnOutcome};

/// A canonical event on the agent's output stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "category", content = "event", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum AgentEvent {
    Message(MessageEvent),
    Tool(ToolEvent),
    Turn(TurnEvent),
    Context(ContextEvent),
    Model(ModelEvent),
}

impl AgentEvent {
    pub fn text(message_id: &str, chunk: &str, state: StreamState) -> Self {
        Self::Message(MessageEvent::Text {
            message_id: message_id.into(),
            chunk: chunk.into(),
            is_complete: state.is_complete(),
        })
    }

    pub fn thought(message_id: &str, chunk: &str, state: StreamState) -> Self {
        Self::Message(MessageEvent::Thought {
            message_id: message_id.into(),
            chunk: chunk.into(),
            is_complete: state.is_complete(),
        })
    }

    pub fn turn_ended(outcome: TurnOutcome) -> Self {
        Self::Turn(TurnEvent::Ended { outcome })
    }

    /// Human-readable text content of this event, if any.
    pub fn content(&self) -> Option<String> {
        match self {
            Self::Message(MessageEvent::Text { chunk, .. } | MessageEvent::Thought { chunk, .. }) => {
                Some(chunk.clone())
            }
            Self::Tool(ToolEvent::Result { result, .. } | ToolEvent::TaskCompleted { result, .. }) => {
                Some(result.result.clone())
            }
            Self::Tool(ToolEvent::Error { error } | ToolEvent::TaskFailed { error, .. }) => Some(error.error.clone()),
            Self::Tool(ToolEvent::TaskCreated { task_id, .. }) => Some(task_id.clone()),
            Self::Tool(ToolEvent::TaskStatus { task_id, status, status_message, .. }) => {
                Some(status_message.as_ref().map_or_else(
                    || format!("{task_id}: {status}"),
                    |message| format!("{task_id}: {status} - {message}"),
                ))
            }
            Self::Tool(ToolEvent::TaskCancelled { task_id, .. }) => Some(format!("{task_id}: cancelled")),
            Self::Context(ContextEvent::CompactionResult { summary, .. }) => Some(summary.clone()),
            _ => None,
        }
    }

    /// The turn's terminal outcome, if this event ends a turn.
    pub fn turn_outcome(&self) -> Option<&TurnOutcome> {
        match self {
            Self::Turn(TurnEvent::Ended { outcome }) => Some(outcome),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{CompactionOutcome, ContextUsage, LlmCallOutcome, LlmCallPurpose};

    #[test]
    fn serializes_nested_event_contract() {
        let event = AgentEvent::text("m1", "hello", StreamState::Complete);
        assert_eq!(
            serde_json::to_value(event).unwrap(),
            serde_json::json!({"category":"message","event":{"type":"text","message_id":"m1","chunk":"hello","is_complete":true}})
        );
    }

    #[test]
    fn nested_events_roundtrip() {
        let events = [
            AgentEvent::text("m", "text", StreamState::Complete),
            AgentEvent::Tool(ToolEvent::DefinitionsUpdated { tools: vec![] }),
            AgentEvent::Turn(TurnEvent::LlmCallEnded {
                purpose: LlmCallPurpose::Chat,
                outcome: LlmCallOutcome::Cancelled,
            }),
            AgentEvent::Context(ContextEvent::UsageUpdated { usage: ContextUsage::default() }),
            AgentEvent::Context(ContextEvent::CompactionEnded { outcome: CompactionOutcome::Completed }),
            AgentEvent::Model(ModelEvent::Switched { previous: "a".into(), new: "b".into() }),
        ];
        for event in events {
            let json = serde_json::to_string(&event).unwrap();
            assert_eq!(serde_json::from_str::<AgentEvent>(&json).unwrap(), event);
        }
    }

    #[test]
    fn turn_outcome_returns_outcome_only_for_turn_end() {
        assert_eq!(AgentEvent::turn_ended(TurnOutcome::Completed).turn_outcome(), Some(&TurnOutcome::Completed));
        assert_eq!(AgentEvent::text("m", "text", StreamState::Complete).turn_outcome(), None);
        assert_eq!(AgentEvent::Turn(TurnEvent::Started { content: vec![] }).turn_outcome(), None);
    }
}
