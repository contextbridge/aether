use acp_utils::notifications::{
    SubAgentEvent, SubAgentToolCallUpdate, SubAgentToolError, SubAgentToolRequest, SubAgentToolResult,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{ContextEvent, MessageEvent, ModelEvent, ToolEvent, TurnEvent};

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
    pub fn text(message_id: &str, chunk: &str, is_complete: bool, model_name: &str) -> Self {
        Self::Message(MessageEvent::Text {
            message_id: message_id.into(),
            chunk: chunk.into(),
            is_complete,
            model_name: model_name.into(),
        })
    }

    pub fn thought(message_id: &str, chunk: &str, is_complete: bool, model_name: &str) -> Self {
        Self::Message(MessageEvent::Thought {
            message_id: message_id.into(),
            chunk: chunk.into(),
            is_complete,
            model_name: model_name.into(),
        })
    }
}

impl From<&AgentEvent> for SubAgentEvent {
    fn from(event: &AgentEvent) -> Self {
        match event {
            AgentEvent::Tool(ToolEvent::Call { request, .. }) => SubAgentEvent::ToolCall {
                request: SubAgentToolRequest {
                    id: request.id.clone(),
                    name: request.name.clone(),
                    arguments: request.arguments.clone(),
                },
            },
            AgentEvent::Tool(ToolEvent::CallUpdate { tool_call_id, chunk, .. }) => SubAgentEvent::ToolCallUpdate {
                update: SubAgentToolCallUpdate { id: tool_call_id.clone(), chunk: chunk.clone() },
            },
            AgentEvent::Tool(ToolEvent::Result { result, result_meta, .. }) => SubAgentEvent::ToolResult {
                result: SubAgentToolResult {
                    id: result.id.clone(),
                    name: result.name.clone(),
                    result_meta: result_meta.clone(),
                },
            },
            AgentEvent::Tool(ToolEvent::Error { error, .. }) => {
                SubAgentEvent::ToolError { error: SubAgentToolError { id: error.id.clone(), name: error.name.clone() } }
            }
            AgentEvent::Turn(TurnEvent::Ended { .. }) => SubAgentEvent::Done,
            _ => SubAgentEvent::Other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{ContextUsage, LlmCallOutcome, LlmCallPurpose};

    #[test]
    fn serializes_nested_event_contract() {
        let event = AgentEvent::text("m1", "hello", true, "claude");
        assert_eq!(
            serde_json::to_value(event).unwrap(),
            serde_json::json!({"category":"message","event":{"type":"text","message_id":"m1","chunk":"hello","is_complete":true,"model_name":"claude"}})
        );
    }

    #[test]
    fn nested_events_roundtrip() {
        let events = [
            AgentEvent::text("m", "text", true, "model"),
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
}
