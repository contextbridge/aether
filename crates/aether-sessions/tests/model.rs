use aether_core::events::{
    AgentEvent, CompactionOutcome, ContextEvent, LlmCallOutcome, StreamState, SubAgentProgressPayload, ToolEvent,
    TurnEvent, TurnOutcome,
};
use aether_sessions::model::last_session_usage;
use aether_sessions::{SessionControlEvent, SessionEvent, UserEvent, last_agent_from_events};
use llm::testing::session_usage_event;
use llm::{LlmCallPurpose, SessionUsageEvent, TokenUsage};

#[test]
fn persisted_event_policy_covers_representative_variants() {
    let retry = SessionEvent::Agent(AgentEvent::Turn(TurnEvent::RetryScheduled {
        purpose: LlmCallPurpose::Chat,
        attempt: 1,
        max_attempts: 3,
        delay_ms: 10,
    }));
    let cancelled = SessionEvent::Agent(AgentEvent::Turn(TurnEvent::Ended { outcome: TurnOutcome::Cancelled }));
    let compaction = SessionEvent::Agent(AgentEvent::Context(ContextEvent::CompactionEnded {
        outcome: CompactionOutcome::Completed,
    }));
    let partial = SessionEvent::Agent(AgentEvent::text("message", "partial", StreamState::Partial));
    let usage = SessionEvent::Agent(AgentEvent::SessionUsage(session_usage(1, 5)));
    let sub_agent_progress = SessionEvent::Agent(AgentEvent::Tool(ToolEvent::SubAgentProgress {
        request: llm::ToolCallRequest { id: "call".into(), name: "spawn".into(), arguments: "{}".into() },
        payload: Box::new(SubAgentProgressPayload {
            task_id: "task_0".into(),
            agent_name: "explorer".into(),
            event: AgentEvent::SessionUsage(session_usage(2, 6)),
        }),
    }));

    assert!(retry.is_persisted());
    assert!(cancelled.is_persisted());
    assert!(compaction.is_persisted());
    assert!(usage.is_persisted());
    assert!(!partial.is_persisted());
    assert!(!sub_agent_progress.is_persisted());
}

#[test]
fn last_session_usage_picks_the_latest_sample_from_a_partial_log() {
    let events = vec![
        SessionEvent::Agent(AgentEvent::SessionUsage(session_usage(1, 5))),
        SessionEvent::Agent(AgentEvent::SessionUsage(session_usage(2, 9))),
        SessionEvent::Agent(AgentEvent::Turn(TurnEvent::Ended {
            outcome: TurnOutcome::Failed { error: "boom".into() },
        })),
    ];

    let last = last_session_usage(&events).expect("usage was logged before the failure");
    assert_eq!(last.sequence, 2);
    assert_eq!(last.totals.tokens.input_tokens.get(), 9);
    assert!(last_session_usage(&[]).is_none());
}

fn session_usage(sequence: u64, total_input_tokens: u64) -> SessionUsageEvent {
    let mut event = session_usage_event(sequence, TokenUsage::new(1, 1));
    event.totals.tokens.input_tokens = llm::Tokens::new(total_input_tokens);
    event
}

#[test]
fn content_helpers_extract_user_text_only() {
    let message = SessionEvent::User(UserEvent::Message { content: vec![llm::ContentBlock::text("Hello")] });

    assert_eq!(message.user_content().as_deref(), Some("Hello"));
    assert_eq!(message.content().as_deref(), Some("Hello"));
    assert!(SessionEvent::User(UserEvent::ClearContext).content().is_none());
}

#[test]
fn event_json_tags_remain_compatible() {
    let event = SessionEvent::User(UserEvent::Message { content: vec![llm::ContentBlock::text("Hello")] });
    let json = serde_json::to_value(&event).unwrap();

    assert_eq!(json["kind"], "user");
    assert_eq!(json["data"]["type"], "message");
    assert_eq!(json["data"]["content"][0]["type"], "text");
    assert_eq!(serde_json::from_value::<SessionEvent>(json).unwrap(), event);
}

#[test]
fn failed_call_diagnostics_survive_session_json_round_trip() {
    let event = SessionEvent::Agent(AgentEvent::Turn(TurnEvent::LlmCallEnded {
        purpose: LlmCallPurpose::Chat,
        outcome: LlmCallOutcome::Failed {
            error: "Server error: boom (status 200, code server_error, request_id req-1)".into(),
            will_retry: true,
            http_status: Some(200),
            provider_request_id: Some("req-1".into()),
            provider_error_code: Some("server_error".into()),
        },
    }));

    let json = serde_json::to_value(&event).unwrap();
    let outcome = &json["data"]["event"]["outcome"];
    assert_eq!(outcome["http_status"], 200);
    assert_eq!(outcome["provider_request_id"], "req-1");
    assert_eq!(outcome["provider_error_code"], "server_error");
    assert_eq!(serde_json::from_value::<SessionEvent>(json).unwrap(), event);
}

#[test]
fn last_agent_uses_the_last_switch() {
    let events = [
        SessionEvent::Control(SessionControlEvent::AgentSwitched { from: None, to: Some("planner".into()) }),
        SessionEvent::Control(SessionControlEvent::AgentSwitched {
            from: Some("planner".into()),
            to: Some("coder".into()),
        }),
    ];

    assert_eq!(last_agent_from_events(Some("default".into()), &events), Some("coder".into()));
    assert_eq!(last_agent_from_events(Some("default".into()), &[]), Some("default".into()));
}
