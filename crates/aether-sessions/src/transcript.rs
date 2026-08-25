use crate::model::{SessionEvent, UserEvent};
use aether_core::events::{AgentEvent, ContextEvent, MessageEvent, ToolEvent, TurnEvent, task_created_result};
use llm::types::IsoString;
use llm::{AssistantReasoning, ChatMessage, Context, ToolCallError, ToolCallResult};

pub fn context_from_events(events: &[SessionEvent]) -> Context {
    let mut context = Context::new(vec![], vec![]);
    let mut acc = TurnAccumulator::default();
    for event in events {
        match event {
            SessionEvent::User(event) => apply_user_event(&mut context, event),
            SessionEvent::Agent(event) => apply_agent_event(&mut context, event, &mut acc),
            SessionEvent::Control(_) => {}
        }
    }
    context
}

pub fn conversation_messages_from_events(events: &[SessionEvent]) -> Vec<ChatMessage> {
    context_from_events(events).messages().iter().filter(|message| !message.is_system()).cloned().collect()
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
        AgentEvent::Tool(ToolEvent::TaskCreated { request, task_id, .. }) => {
            acc.tool_results.push(Ok(task_created_result(request, task_id)));
        }
        AgentEvent::Tool(ToolEvent::Error { error }) => {
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
        AgentEvent::Tool(
            event @ (ToolEvent::TaskCompleted { .. } | ToolEvent::TaskFailed { .. } | ToolEvent::TaskCancelled { .. }),
        ) => {
            if let Some(message) = event.task_context_message() {
                ctx.add_message(message);
            }
        }
        _ => {}
    }
}
