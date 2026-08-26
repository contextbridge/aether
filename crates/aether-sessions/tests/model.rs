use aether_core::events::{
    AgentEvent, CompactionOutcome, ContextEvent, LlmCallPurpose, StreamState, TurnEvent, TurnOutcome,
};
use aether_sessions::{SessionControlEvent, SessionEvent, UserEvent, last_agent_from_events};

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

    assert!(retry.is_persisted());
    assert!(cancelled.is_persisted());
    assert!(compaction.is_persisted());
    assert!(!partial.is_persisted());
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
