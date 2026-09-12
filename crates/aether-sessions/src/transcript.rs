use crate::model::{SessionEvent, UserEvent};
use aether_core::events::{AgentEvent, ContextEvent, MessageEvent, ToolEvent, TurnEvent, task_created_result};
use llm::{AssistantReasoning, ChatMessage, Context, MessageId, ToolCallError, ToolCallResult};

pub fn context_from_events(events: &[SessionEvent]) -> Context {
    let mut context = Context::new(vec![], vec![]);
    let mut acc = MessageAccumulator::default();
    for event in events {
        match event {
            SessionEvent::User(event) => {
                acc.flush(&mut context);
                apply_user_event(&mut context, event);
            }
            SessionEvent::Agent(event) => apply_agent_event(&mut context, event, &mut acc),
            SessionEvent::Control(_) => {}
        }
    }
    acc.flush(&mut context);
    context
}

pub fn conversation_messages_from_events(events: &[SessionEvent]) -> Vec<ChatMessage> {
    context_from_events(events).messages().iter().filter(|message| !message.is_system()).cloned().collect()
}

#[derive(Default)]
struct MessageAccumulator {
    message_id: Option<MessageId>,
    text: String,
    reasoning: String,
    tool_results: Vec<Result<ToolCallResult, ToolCallError>>,
    task_messages: Vec<ChatMessage>,
}

impl MessageAccumulator {
    fn flush(&mut self, context: &mut Context) {
        let pending = std::mem::take(self);
        if let Some(message_id) = pending.message_id {
            let reasoning = AssistantReasoning::from_parts(pending.reasoning, None);
            context.push_assistant_turn(message_id, &pending.text, reasoning, pending.tool_results);
        }
        for message in pending.task_messages {
            context.add_message(message);
        }
    }
}

fn apply_user_event(ctx: &mut Context, event: &UserEvent) {
    match event {
        UserEvent::Message { message_id, content } => {
            ctx.add_message(ChatMessage::user_with_id(message_id.clone(), content.clone()));
        }
        UserEvent::ClearContext => ctx.clear_conversation(),
    }
}

fn apply_agent_event(ctx: &mut Context, event: &AgentEvent, acc: &mut MessageAccumulator) {
    match event {
        AgentEvent::Message(MessageEvent::Text { message_id, chunk, is_complete: true }) => {
            if acc.message_id.is_some() {
                acc.flush(ctx);
            }
            acc.message_id = Some(message_id.clone());
            acc.text.clone_from(chunk);
        }
        AgentEvent::Message(MessageEvent::Thought { message_id, chunk, is_complete: true }) => {
            if acc.message_id.as_ref().is_some_and(|id| id.thought() == *message_id) {
                acc.reasoning.clone_from(chunk);
            }
        }
        AgentEvent::Tool(ToolEvent::Call { .. }) => {
            if acc.message_id.is_some() {
                acc.flush(ctx);
            }
        }
        AgentEvent::Tool(ToolEvent::Result { result, .. }) => acc.tool_results.push(Ok(result.clone())),
        AgentEvent::Tool(ToolEvent::TaskCreated { request, task_id, .. }) => {
            acc.tool_results.push(Ok(task_created_result(request, task_id)));
        }
        AgentEvent::Tool(ToolEvent::Error { error }) => acc.tool_results.push(Err(error.clone())),
        AgentEvent::Turn(TurnEvent::AutoContinue { message_id, content, .. }) => {
            acc.flush(ctx);
            ctx.add_message(ChatMessage::user_with_id(message_id.clone(), content.clone()));
        }
        AgentEvent::Turn(TurnEvent::Ended { .. }) => acc.flush(ctx),
        AgentEvent::Context(ContextEvent::Cleared) => {
            ctx.clear_conversation();
            *acc = MessageAccumulator::default();
        }
        AgentEvent::Context(ContextEvent::CompactionResult { message_id, summary, .. }) => {
            acc.flush(ctx);
            *ctx = ctx.with_compacted_summary(message_id.clone(), summary);
        }
        AgentEvent::Tool(
            event @ (ToolEvent::TaskCompleted { .. } | ToolEvent::TaskFailed { .. } | ToolEvent::TaskCancelled { .. }),
        ) => {
            if let Some(message) = event.task_context_message() {
                if acc.message_id.is_none() && !acc.tool_results.is_empty() {
                    acc.task_messages.push(message);
                } else {
                    acc.flush(ctx);
                    ctx.add_message(message);
                }
            }
        }
        _ => {}
    }
}
