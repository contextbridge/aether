use crate::events::{AgentEvent, ContextEvent, MessageEvent, ToolEvent, TurnEvent};
use crate::session::{SessionEvent, UserEvent};
use llm::types::IsoString;
use llm::{AssistantReasoning, ChatMessage, Context, ToolCallError, ToolCallResult};

pub fn conversation_messages_from_events(events: &[SessionEvent]) -> Vec<ChatMessage> {
    Context::from_events(events).messages().iter().filter(|message| !message.is_system()).cloned().collect()
}

pub trait ContextExt {
    fn from_events(events: &[SessionEvent]) -> Self
    where
        Self: Sized;
}

impl ContextExt for Context {
    fn from_events(events: &[SessionEvent]) -> Self {
        let mut context = Context::new(vec![], vec![]);
        let mut acc = TurnAccumulator::default();
        for event in events {
            match event {
                SessionEvent::User(e) => apply_user_event(&mut context, e),
                SessionEvent::Agent(m) => apply_agent_event(&mut context, m, &mut acc),
                SessionEvent::Control(_) => {}
            }
        }
        context
    }
}

#[derive(Default)]
struct TurnAccumulator {
    text: String,
    reasoning: String,
    tool_results: Vec<Result<ToolCallResult, ToolCallError>>,
}

fn apply_user_event(ctx: &mut Context, event: &UserEvent) {
    match event {
        UserEvent::Message { content } => {
            ctx.add_message(ChatMessage::User { content: content.clone(), timestamp: IsoString::now() });
        }
        UserEvent::ClearContext => {
            ctx.clear_conversation();
        }
    }
}

fn apply_agent_event(ctx: &mut Context, event: &AgentEvent, acc: &mut TurnAccumulator) {
    match event {
        AgentEvent::Message(MessageEvent::Text { chunk, is_complete: true, .. }) => {
            acc.text.clone_from(chunk);
        }
        AgentEvent::Message(MessageEvent::Thought { chunk, is_complete: true, .. }) => {
            acc.reasoning.clone_from(chunk);
        }
        AgentEvent::Tool(ToolEvent::Result { result, .. }) => {
            acc.tool_results.push(Ok(result.clone()));
        }
        AgentEvent::Tool(ToolEvent::Error { error, .. }) => {
            acc.tool_results.push(Err(error.clone()));
        }
        AgentEvent::Turn(TurnEvent::Ended { .. }) => {
            let text = std::mem::take(&mut acc.text);
            let reasoning_text = std::mem::take(&mut acc.reasoning);
            let tools = std::mem::take(&mut acc.tool_results);
            if !text.is_empty() || !tools.is_empty() {
                let reasoning = AssistantReasoning::from_parts(reasoning_text, None);
                ctx.push_assistant_turn(&text, reasoning, tools);
            }
        }
        AgentEvent::Context(ContextEvent::Cleared) => {
            ctx.clear_conversation();
            acc.text.clear();
            acc.reasoning.clear();
            acc.tool_results.clear();
        }
        AgentEvent::Context(ContextEvent::CompactionResult { summary, .. }) => {
            *ctx = ctx.with_compacted_summary(summary);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use crate::events::TurnOutcome;
    use crate::session::{SessionControlEvent, last_agent_from_events};

    use super::*;
    use llm::ToolCallResult;

    fn system_context() -> Context {
        Context::new(
            vec![ChatMessage::System { content: "You are helpful.".to_string(), timestamp: IsoString::now() }],
            vec![],
        )
    }

    fn user_msg(content: &str) -> UserEvent {
        UserEvent::Message { content: vec![llm::ContentBlock::text(content)] }
    }

    fn user_session(content: &str) -> SessionEvent {
        SessionEvent::User(user_msg(content))
    }

    fn text_complete(chunk: &str) -> AgentEvent {
        AgentEvent::text("msg_1", chunk, true)
    }

    fn tool_result(id: &str, name: &str, result: &str) -> AgentEvent {
        AgentEvent::Tool(ToolEvent::Result {
            result: ToolCallResult {
                id: id.to_string(),
                name: name.to_string(),
                arguments: "{}".to_string(),
                result: result.to_string(),
            },
            result_meta: None,
        })
    }

    fn agent_session(msg: AgentEvent) -> SessionEvent {
        SessionEvent::Agent(msg)
    }

    /// Runs a sequence of agent events against a `system_context` and returns the context.
    fn run_agent_events(events: &[AgentEvent]) -> Context {
        let mut ctx = system_context();
        let mut acc = TurnAccumulator::default();
        for event in events {
            apply_agent_event(&mut ctx, event, &mut acc);
        }
        ctx
    }

    #[test]
    fn from_events_ignores_control_events() {
        let ctx = Context::from_events(&[
            user_session("Hello"),
            SessionEvent::Control(SessionControlEvent::AgentSwitched {
                from: Some("Planner".to_string()),
                to: Some("Coder".to_string()),
            }),
            agent_session(text_complete("Hi there!")),
            agent_session(AgentEvent::turn_ended(TurnOutcome::Completed)),
        ]);

        assert_eq!(ctx.message_count(), 2);
        assert!(matches!(ctx.messages()[0], ChatMessage::User { .. }));
        assert!(matches!(ctx.messages()[1], ChatMessage::Assistant { .. }));
    }

    #[test]
    fn conversation_messages_from_events_filters_system_messages() {
        let messages = conversation_messages_from_events(&[
            user_session("Hello"),
            agent_session(text_complete("Hi there!")),
            agent_session(AgentEvent::turn_ended(TurnOutcome::Completed)),
        ]);

        assert_eq!(messages.len(), 2);
        assert!(messages.iter().all(|message| !message.is_system()));
    }

    #[test]
    fn last_agent_from_events_returns_last_switched_agent() {
        let events = [
            SessionEvent::Control(SessionControlEvent::AgentSwitched { from: None, to: Some("Planner".to_string()) }),
            user_session("Hello"),
            SessionEvent::Control(SessionControlEvent::AgentSwitched {
                from: Some("Planner".to_string()),
                to: Some("Coder".to_string()),
            }),
        ];

        assert_eq!(last_agent_from_events(Some("Default".to_string()), &events), Some("Coder".to_string()));
    }

    #[test]
    fn apply_user_message_adds_user_message() {
        let mut ctx = system_context();
        apply_user_event(&mut ctx, &user_msg("Hello"));
        assert_eq!(ctx.message_count(), 2);
        match &ctx.messages()[1] {
            ChatMessage::User { content, .. } => {
                assert_eq!(content, &vec![llm::ContentBlock::text("Hello")]);
            }
            other => panic!("Expected User, got {other:?}"),
        }
    }

    #[test]
    fn apply_user_clear_retains_system_messages() {
        let mut ctx = system_context();
        apply_user_event(&mut ctx, &user_msg("Hello"));
        apply_user_event(&mut ctx, &UserEvent::ClearContext);
        assert_eq!(ctx.message_count(), 1);
        assert!(ctx.messages()[0].is_system());
    }

    #[test]
    fn apply_agent_produces_assistant_and_tool_results() {
        let ctx = run_agent_events(&[
            tool_result("call_1", "read_file", "file contents"),
            text_complete("Here is the file"),
            AgentEvent::turn_ended(TurnOutcome::Completed),
        ]);

        assert_eq!(ctx.message_count(), 3);
        match &ctx.messages()[1] {
            ChatMessage::Assistant { content, tool_calls, .. } => {
                assert_eq!(content, "Here is the file");
                assert_eq!(tool_calls.len(), 1);
                assert_eq!(tool_calls[0].name, "read_file");
            }
            other => panic!("Expected Assistant, got {other:?}"),
        }
        assert!(ctx.messages()[2].is_tool_result());
    }

    #[test]
    fn apply_agent_context_cleared() {
        let mut ctx = system_context();
        let mut acc = TurnAccumulator::default();
        apply_user_event(&mut ctx, &user_msg("Hello"));
        apply_agent_event(&mut ctx, &AgentEvent::Context(ContextEvent::Cleared), &mut acc);
        assert_eq!(ctx.message_count(), 1);
        assert!(ctx.messages()[0].is_system());
    }

    #[test]
    fn apply_agent_compaction_replaces_with_summary() {
        let mut ctx = system_context();
        let mut acc = TurnAccumulator::default();
        apply_user_event(&mut ctx, &user_msg("Hello"));
        apply_agent_event(
            &mut ctx,
            &AgentEvent::Context(ContextEvent::CompactionResult {
                summary: "Summary of conversation".to_string(),
                messages_removed: 1,
            }),
            &mut acc,
        );
        assert_eq!(ctx.message_count(), 2);
        assert!(ctx.messages()[0].is_system());
        assert!(ctx.messages()[1].is_summary());
    }

    #[test]
    fn done_without_content_does_not_add_message() {
        let ctx = run_agent_events(&[AgentEvent::turn_ended(TurnOutcome::Completed)]);
        assert_eq!(ctx.message_count(), 1);
    }

    #[test]
    fn streaming_chunks_are_ignored() {
        let ctx = run_agent_events(&[AgentEvent::text("msg_1", "partial", false)]);
        assert_eq!(ctx.message_count(), 1);
    }

    #[test]
    fn accumulator_resets_after_done() {
        let ctx = run_agent_events(&[
            text_complete("Turn 1"),
            AgentEvent::turn_ended(TurnOutcome::Completed),
            AgentEvent::text("msg_2", "Turn 2", true),
            AgentEvent::turn_ended(TurnOutcome::Completed),
        ]);
        assert_eq!(ctx.message_count(), 3);
    }

    #[test]
    fn user_event_serde_roundtrip() {
        let cases: Vec<UserEvent> = vec![user_msg("Hello"), UserEvent::ClearContext];
        for event in cases {
            let json = serde_json::to_string(&event).unwrap();
            let parsed: UserEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, event);
        }
    }

    #[test]
    fn from_events_basic_conversation() {
        let ctx = Context::from_events(&[
            user_session("Hello"),
            agent_session(text_complete("Hi there!")),
            agent_session(AgentEvent::turn_ended(TurnOutcome::Completed)),
        ]);
        assert_eq!(ctx.message_count(), 2);
        assert!(matches!(ctx.messages()[0], ChatMessage::User { .. }));
        assert!(matches!(ctx.messages()[1], ChatMessage::Assistant { .. }));
    }

    #[test]
    fn from_events_with_tool_calls() {
        let ctx = Context::from_events(&[
            user_session("Read Cargo.toml"),
            agent_session(AgentEvent::Tool(ToolEvent::Call {
                request: llm::ToolCallRequest {
                    id: "call_1".to_string(),
                    name: "read_file".to_string(),
                    arguments: "{}".to_string(),
                },
            })),
            agent_session(tool_result("call_1", "read_file", "file contents")),
            agent_session(text_complete("Here is the file")),
            agent_session(AgentEvent::turn_ended(TurnOutcome::Completed)),
        ]);

        assert_eq!(ctx.message_count(), 3);
        match &ctx.messages()[1] {
            ChatMessage::Assistant { tool_calls, .. } => {
                assert_eq!(tool_calls.len(), 1);
                assert_eq!(tool_calls[0].name, "read_file");
            }
            other => panic!("Expected Assistant, got {other:?}"),
        }
        assert!(ctx.messages()[2].is_tool_result());
    }

    #[test]
    fn from_events_handles_clear() {
        let ctx = Context::from_events(&[
            user_session("Hello"),
            agent_session(text_complete("Hi!")),
            agent_session(AgentEvent::turn_ended(TurnOutcome::Completed)),
            SessionEvent::User(UserEvent::ClearContext),
            user_session("Start fresh"),
        ]);
        assert_eq!(ctx.message_count(), 1);
        assert!(matches!(ctx.messages()[0], ChatMessage::User { .. }));
    }

    #[test]
    fn from_events_handles_compaction() {
        let ctx = Context::from_events(&[
            user_session("Hello"),
            agent_session(text_complete("Hi!")),
            agent_session(AgentEvent::turn_ended(TurnOutcome::Completed)),
            agent_session(AgentEvent::Context(ContextEvent::CompactionResult {
                summary: "Earlier we greeted each other.".to_string(),
                messages_removed: 2,
            })),
            user_session("What did we talk about?"),
        ]);
        assert_eq!(ctx.message_count(), 2);
        assert!(ctx.messages()[0].is_summary());
    }

    #[test]
    fn from_events_empty() {
        let ctx = Context::from_events(&[]);
        assert_eq!(ctx.message_count(), 0);
    }
}
