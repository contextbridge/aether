use crate::events::{AgentEvent, ContextEvent, ToolEvent, TurnEvent, task_created_result};
use llm::{AssistantReply, ChatMessage, ContentBlock, Context, LlmModel, MessageId, ToolCallError, ToolCallResult};

#[derive(Debug, Default)]
pub struct ContextRecorder {
    tool_results: Vec<Result<ToolCallResult, ToolCallError>>,
    task_messages: Vec<ChatMessage>,
}

impl ContextRecorder {
    pub fn has_tool_results(&self) -> bool {
        !self.tool_results.is_empty()
    }

    pub fn reply(&self, message_id: MessageId, model: Option<LlmModel>, content: AssistantReply) -> ChatMessage {
        ChatMessage::assistant_reply(message_id, model, content, &self.tool_results)
    }

    pub fn record(&mut self, context: &mut Context, event: &AgentEvent) {
        match event {
            AgentEvent::Context(ContextEvent::ReplyRecorded { message }) => self.record_reply(context, message),
            AgentEvent::Tool(ToolEvent::Result { result, .. }) => self.tool_results.push(Ok(result.clone())),
            AgentEvent::Tool(ToolEvent::TaskCreated { request, task_id, .. }) => {
                self.tool_results.push(Ok(task_created_result(request, task_id)));
            }
            AgentEvent::Tool(ToolEvent::Error { error }) => self.tool_results.push(Err(error.clone())),
            AgentEvent::Tool(
                event @ (ToolEvent::TaskCompleted { .. }
                | ToolEvent::TaskFailed { .. }
                | ToolEvent::TaskCancelled { .. }),
            ) => {
                if let Some(message) = event.task_context_message() {
                    self.record_task_message(context, message);
                }
            }
            AgentEvent::Turn(TurnEvent::AutoContinue { message_id, content, .. }) => {
                self.record_user_message(context, message_id.clone(), content.clone());
            }
            AgentEvent::Turn(TurnEvent::Ended { .. }) => self.flush(context),
            AgentEvent::Context(ContextEvent::Cleared) => self.clear(context),
            AgentEvent::Context(ContextEvent::CompactionResult { message_id, summary, .. }) => {
                self.flush(context);
                *context = context.with_compacted_summary(message_id.clone(), summary);
            }
            _ => {}
        }
    }

    pub fn record_user_message(&mut self, context: &mut Context, message_id: MessageId, blocks: Vec<ContentBlock>) {
        self.flush(context);
        context.add_message(ChatMessage::user_with_id(message_id, blocks));
    }

    fn clear(&mut self, context: &mut Context) {
        *self = Self::default();
        context.clear_conversation();
    }

    pub fn replace(&mut self, context: &mut Context, messages: Vec<ChatMessage>) {
        *self = Self::default();
        context.replace_conversation(messages);
    }

    pub fn flush(&mut self, context: &mut Context) {
        self.tool_results.clear();
        for message in self.task_messages.drain(..) {
            context.add_message(message);
        }
    }

    fn record_reply(&mut self, context: &mut Context, reply: &ChatMessage) {
        context.add_message(reply.clone());
        for result in self.tool_results.drain(..) {
            context.add_message(ChatMessage::ToolCallResult(result));
        }
        for message in self.task_messages.drain(..) {
            context.add_message(message);
        }
    }

    fn record_task_message(&mut self, context: &mut Context, message: ChatMessage) {
        if self.tool_results.is_empty() {
            context.add_message(message);
        } else {
            self.task_messages.push(message);
        }
    }
}
