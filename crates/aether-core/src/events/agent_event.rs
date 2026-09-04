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
    SessionUsage(llm::SessionUsageEvent),
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
    use crate::events::{CompactionOutcome, LlmCallOutcome};
    use llm::{ContextUsage, LlmCallPurpose};

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
            AgentEvent::SessionUsage(llm::testing::session_usage_event(1, llm::TokenUsage::new(1, 2))),
            AgentEvent::Tool(ToolEvent::DisplayUpdate {
                request: llm::ToolCallRequest { id: "call".into(), name: "read".into(), arguments: "{}".into() },
                meta: mcp_utils::display_meta::ToolDisplayMeta::new("Read file", "main.rs").into(),
            }),
            AgentEvent::Tool(ToolEvent::SubAgentProgress {
                request: llm::ToolCallRequest { id: "call".into(), name: "spawn".into(), arguments: "{}".into() },
                payload: Box::new(crate::events::SubAgentProgressPayload {
                    task_id: "task_0".into(),
                    agent_name: "explorer".into(),
                    event: AgentEvent::turn_ended(TurnOutcome::Completed),
                }),
            }),
        ];
        for event in events {
            let json = serde_json::to_string(&event).unwrap();
            assert_eq!(serde_json::from_str::<AgentEvent>(&json).unwrap(), event);
        }
    }

    #[test]
    fn failed_call_ended_serializes_diagnostics_and_omits_absent_fields() {
        let event = AgentEvent::Turn(TurnEvent::LlmCallEnded {
            purpose: LlmCallPurpose::Chat,
            outcome: LlmCallOutcome::Failed {
                error: "Server error: boom (status 200, code server_error, request_id req-1)".into(),
                will_retry: true,
                http_status: Some(200),
                provider_request_id: Some("req-1".into()),
                provider_error_code: Some("server_error".into()),
            },
        });
        assert_eq!(
            serde_json::to_value(&event).unwrap(),
            serde_json::json!({
                "category": "turn",
                "event": {
                    "type": "llm_call_ended",
                    "purpose": "chat",
                    "outcome": {
                        "status": "failed",
                        "error": "Server error: boom (status 200, code server_error, request_id req-1)",
                        "will_retry": true,
                        "http_status": 200,
                        "provider_request_id": "req-1",
                        "provider_error_code": "server_error"
                    }
                }
            })
        );

        let minimal = AgentEvent::Turn(TurnEvent::LlmCallEnded {
            purpose: LlmCallPurpose::Chat,
            outcome: LlmCallOutcome::failed("boom", false),
        });
        let value = serde_json::to_value(&minimal).unwrap();
        let outcome = &value["event"]["outcome"];
        assert_eq!(outcome["status"], "failed");
        assert!(outcome.get("http_status").is_none());
        assert!(outcome.get("provider_request_id").is_none());
        assert!(outcome.get("provider_error_code").is_none());
        assert_eq!(serde_json::from_value::<AgentEvent>(value).unwrap(), minimal);
    }

    #[test]
    fn from_llm_error_copies_provider_diagnostics() {
        let provider = llm::ProviderError::server("boom")
            .with_http_status(503)
            .with_request_id(Some("req-9".into()))
            .with_code(Some("server_error".into()));
        let outcome = LlmCallOutcome::from_llm_error(&llm::LlmError::from(provider), true);
        match outcome {
            LlmCallOutcome::Failed { http_status, provider_request_id, provider_error_code, will_retry, .. } => {
                assert!(will_retry);
                assert_eq!(http_status, Some(503));
                assert_eq!(provider_request_id.as_deref(), Some("req-9"));
                assert_eq!(provider_error_code.as_deref(), Some("server_error"));
            }
            _ => panic!("expected failed outcome"),
        }
        let terminal = LlmCallOutcome::from_llm_error(&llm::LlmError::InvalidArgument("bad".into()), false);
        match terminal {
            LlmCallOutcome::Failed { http_status, provider_request_id, provider_error_code, .. } => {
                assert_eq!(http_status, None);
                assert_eq!(provider_request_id, None);
                assert_eq!(provider_error_code, None);
            }
            _ => panic!("expected failed outcome"),
        }
    }

    #[test]
    fn turn_outcome_returns_outcome_only_for_turn_end() {
        assert_eq!(AgentEvent::turn_ended(TurnOutcome::Completed).turn_outcome(), Some(&TurnOutcome::Completed));
        assert_eq!(AgentEvent::text("m", "text", StreamState::Complete).turn_outcome(), None);
        assert_eq!(AgentEvent::Turn(TurnEvent::Started { content: vec![] }).turn_outcome(), None);
    }
}
