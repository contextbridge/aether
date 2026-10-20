use crate::model::{SessionEvent, UserEvent};
use aether_core::events::{AgentEvent, ContextEvent, ToolEvent, TurnEvent, task_created_result};
use llm::{ChatMessage, Context, ToolCallError, ToolCallResult};

pub fn context_from_events(events: &[SessionEvent]) -> Context {
    let mut context = Context::new(vec![], vec![]);
    let mut pending = PendingTurn::default();
    for event in events {
        match event {
            SessionEvent::User(event) => {
                pending.flush(&mut context);
                apply_user_event(&mut context, event);
            }
            SessionEvent::Agent(event) => apply_agent_event(&mut context, event, &mut pending),
            SessionEvent::Control(_) => {}
        }
    }
    pending.flush(&mut context);
    context
}

pub fn conversation_messages_from_events(events: &[SessionEvent]) -> Vec<ChatMessage> {
    context_from_events(events).messages().iter().filter(|message| !message.is_system()).cloned().collect()
}

/// Tool results are logged before the reply that requested them is recorded,
/// and task updates that land in between follow that reply into the context.
#[derive(Default)]
struct PendingTurn {
    tool_results: Vec<Result<ToolCallResult, ToolCallError>>,
    task_messages: Vec<ChatMessage>,
}

impl PendingTurn {
    fn record(&mut self, context: &mut Context, reply: &ChatMessage) {
        let pending = std::mem::take(self);
        context.add_message(reply.clone());
        for result in pending.tool_results {
            context.add_message(ChatMessage::ToolCallResult(result));
        }
        for message in pending.task_messages {
            context.add_message(message);
        }
    }

    /// Results whose reply was never recorded are dropped along with it.
    fn flush(&mut self, context: &mut Context) {
        let pending = std::mem::take(self);
        for message in pending.task_messages {
            context.add_message(message);
        }
    }
}

fn apply_user_event(ctx: &mut Context, event: &UserEvent) {
    match event {
        UserEvent::Message { message_id, content, .. } => {
            ctx.add_message(ChatMessage::user_with_id(message_id.clone(), content.clone()));
        }
        UserEvent::ClearContext => ctx.clear_conversation(),
    }
}

fn apply_agent_event(ctx: &mut Context, event: &AgentEvent, pending: &mut PendingTurn) {
    match event {
        AgentEvent::Context(ContextEvent::ReplyRecorded { message }) => pending.record(ctx, message),
        AgentEvent::Tool(ToolEvent::Result { result, .. }) => pending.tool_results.push(Ok(result.clone())),
        AgentEvent::Tool(ToolEvent::TaskCreated { request, task_id, .. }) => {
            pending.tool_results.push(Ok(task_created_result(request, task_id)));
        }
        AgentEvent::Tool(ToolEvent::Error { error }) => pending.tool_results.push(Err(error.clone())),
        AgentEvent::Turn(TurnEvent::AutoContinue { message_id, content, .. }) => {
            pending.flush(ctx);
            ctx.add_message(ChatMessage::user_with_id(message_id.clone(), content.clone()));
        }
        AgentEvent::Turn(TurnEvent::Ended { .. }) => pending.flush(ctx),
        AgentEvent::Context(ContextEvent::Cleared) => {
            ctx.clear_conversation();
            *pending = PendingTurn::default();
        }
        AgentEvent::Context(ContextEvent::CompactionResult { message_id, summary, .. }) => {
            pending.flush(ctx);
            *ctx = ctx.with_compacted_summary(message_id.clone(), summary);
        }
        AgentEvent::Tool(
            event @ (ToolEvent::TaskCompleted { .. } | ToolEvent::TaskFailed { .. } | ToolEvent::TaskCancelled { .. }),
        ) => {
            if let Some(message) = event.task_context_message() {
                if pending.tool_results.is_empty() {
                    ctx.add_message(message);
                } else {
                    pending.task_messages.push(message);
                }
            }
        }
        _ => {}
    }
}
