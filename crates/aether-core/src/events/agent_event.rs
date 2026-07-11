use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{ContextEvent, MessageEvent, ModelEvent, ToolEvent, TurnEvent, TurnOutcome};

/// A canonical event on the agent's output stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "category", content = "event", rename_all = "snake_case")]
pub enum AgentEvent {
    Message(MessageEvent),
    Tool(ToolEvent),
    Turn(TurnEvent),
    Context(ContextEvent),
    Model(ModelEvent),
}

impl AgentEvent {
    pub fn text(message_id: &str, chunk: &str, is_complete: bool) -> Self {
        Self::Message(MessageEvent::Text { message_id: message_id.into(), chunk: chunk.into(), is_complete })
    }

    pub fn thought(message_id: &str, chunk: &str, is_complete: bool) -> Self {
        Self::Message(MessageEvent::Thought { message_id: message_id.into(), chunk: chunk.into(), is_complete })
    }

    pub fn turn_ended(outcome: TurnOutcome) -> Self {
        Self::Turn(TurnEvent::Ended { outcome })
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
    use crate::events::{ContextUsage, LlmCallOutcome, LlmCallPurpose};

    #[test]
    fn serializes_nested_event_contract() {
        let event = AgentEvent::text("m1", "hello", true);
        assert_eq!(
            serde_json::to_value(event).unwrap(),
            serde_json::json!({"category":"message","event":{"type":"text","message_id":"m1","chunk":"hello","is_complete":true}})
        );
    }

    #[test]
    fn nested_events_roundtrip() {
        let events = [
            AgentEvent::text("m", "text", true),
            AgentEvent::Tool(ToolEvent::DefinitionsUpdated { tools: vec![] }),
            AgentEvent::Turn(TurnEvent::LlmCallEnded {
                purpose: LlmCallPurpose::Chat,
                outcome: LlmCallOutcome::Cancelled,
            }),
            AgentEvent::Context(ContextEvent::UsageUpdated { usage: ContextUsage::default() }),
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
        assert_eq!(AgentEvent::text("m", "text", true).turn_outcome(), None);
        assert_eq!(AgentEvent::Turn(TurnEvent::Started).turn_outcome(), None);
    }
}
