//! Shared session fixtures for tests, mirroring `llm::testing` and `aether_core::testing`.

use crate::{SessionControlEvent, SessionEvent, SessionMeta, UserEvent};
use aether_core::events::{AgentEvent, ContextEvent, StreamState, ToolEvent, TurnEvent, TurnOutcome};
use llm::{ContentBlock, ContextUsage, Tokens, ToolCallError, ToolCallRequest, ToolCallResult};
use std::path::PathBuf;

pub fn session_meta(id: &str) -> SessionMetaBuilder {
    SessionMetaBuilder::new(id)
}

pub struct SessionMetaBuilder {
    meta: SessionMeta,
}

impl SessionMetaBuilder {
    fn new(id: &str) -> Self {
        Self {
            meta: SessionMeta {
                session_id: id.to_string(),
                cwd: PathBuf::from("/tmp/project"),
                model: "test-model".to_string(),
                selected_mode: Some("planner".to_string()),
                created_at: "2026-01-01T00:00:00Z".to_string(),
            },
        }
    }

    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.meta.cwd = cwd.into();
        self
    }

    pub fn model(mut self, model: &str) -> Self {
        self.meta.model = model.to_string();
        self
    }

    pub fn mode(mut self, mode: Option<&str>) -> Self {
        self.meta.selected_mode = mode.map(str::to_string);
        self
    }

    pub fn created_at(mut self, created_at: &str) -> Self {
        self.meta.created_at = created_at.to_string();
        self
    }

    pub fn build(self) -> SessionMeta {
        self.meta
    }
}

pub fn user_message(text: &str) -> SessionEvent {
    SessionEvent::User(UserEvent::Message { content: vec![ContentBlock::text(text)] })
}

pub fn assistant_text(message_id: &str, text: &str) -> SessionEvent {
    SessionEvent::Agent(AgentEvent::text(message_id, text, StreamState::Complete))
}

pub fn assistant_chunk(message_id: &str, chunk: &str) -> SessionEvent {
    SessionEvent::Agent(AgentEvent::text(message_id, chunk, StreamState::Partial))
}

pub fn turn_ended() -> SessionEvent {
    SessionEvent::Agent(AgentEvent::Turn(TurnEvent::Ended { outcome: TurnOutcome::Completed }))
}

pub fn agent_switched(from: Option<&str>, to: Option<&str>) -> SessionEvent {
    SessionEvent::Control(SessionControlEvent::AgentSwitched {
        from: from.map(str::to_string),
        to: to.map(str::to_string),
    })
}

pub fn tool_call(id: &str, name: &str) -> SessionEvent {
    SessionEvent::Agent(AgentEvent::Tool(ToolEvent::Call {
        request: ToolCallRequest { id: id.to_string(), name: name.to_string(), arguments: "{}".to_string() },
    }))
}

pub fn tool_result(id: &str, name: &str, result: &str) -> SessionEvent {
    SessionEvent::Agent(AgentEvent::Tool(ToolEvent::Result {
        result: ToolCallResult {
            id: id.to_string(),
            name: name.to_string(),
            arguments: "{}".to_string(),
            result: result.to_string(),
        },
        result_meta: None,
    }))
}

pub fn tool_error(id: &str, name: &str, error: &str) -> SessionEvent {
    SessionEvent::Agent(AgentEvent::Tool(ToolEvent::Error {
        error: ToolCallError {
            id: id.to_string(),
            name: name.to_string(),
            arguments: Some("{}".to_string()),
            error: error.to_string(),
        },
    }))
}

pub fn context_usage(usage_ratio: f64) -> SessionEvent {
    SessionEvent::Agent(AgentEvent::Context(ContextEvent::UsageUpdated {
        usage: ContextUsage {
            usage_ratio: Some(usage_ratio),
            context_limit: Some(Tokens::new(100)),
            input_tokens: Tokens::new(1),
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_meta_builder_overrides_only_requested_fields() {
        let meta = session_meta("s1").cwd("/repo").model("m").mode(None).created_at("t0").build();

        assert_eq!(meta.session_id, "s1");
        assert_eq!(meta.cwd, PathBuf::from("/repo"));
        assert_eq!(meta.model, "m");
        assert_eq!(meta.selected_mode, None);
        assert_eq!(meta.created_at, "t0");

        let defaults = session_meta("s2").build();
        assert_eq!(defaults.cwd, PathBuf::from("/tmp/project"));
        assert_eq!(defaults.model, "test-model");
        assert_eq!(defaults.selected_mode, Some("planner".to_string()));
        assert_eq!(defaults.created_at, "2026-01-01T00:00:00Z");
    }

    #[test]
    fn event_builders_round_trip_through_the_session_wire_format() {
        let events = [
            user_message("Hello"),
            assistant_text("message-1", "Hi"),
            assistant_chunk("message-1", "Hi"),
            turn_ended(),
            tool_call("call-1", "read"),
            tool_result("call-1", "read", "ok"),
            tool_error("call-1", "read", "failed"),
            agent_switched(None, Some("coder")),
            context_usage(0.5),
        ];

        for event in events {
            let json = serde_json::to_value(&event).expect("event serializes");
            let parsed: SessionEvent = serde_json::from_value(json).expect("event deserializes");
            assert_eq!(parsed, event);
        }

        assert!(assistant_text("message-1", "Hi").is_persisted());
        assert!(!assistant_chunk("message-1", "Hi").is_persisted());
    }
}
