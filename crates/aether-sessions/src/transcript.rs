use crate::model::{SessionEvent, UserEvent};
use aether_core::context::ContextRecorder;
use llm::{ChatMessage, Context};

pub fn context_from_events(events: &[SessionEvent]) -> Context {
    let mut context = Context::new(vec![], vec![]);
    let mut recorder = ContextRecorder::default();
    for event in events {
        match event {
            SessionEvent::User(UserEvent::Message { message_id, content, .. }) => {
                recorder.record_user_message(&mut context, message_id.clone(), content.clone());
            }
            SessionEvent::Agent(event) => recorder.record(&mut context, event),
            SessionEvent::Control(_) => {}
        }
    }
    recorder.flush(&mut context);
    context
}

pub fn conversation_messages_from_events(events: &[SessionEvent]) -> Vec<ChatMessage> {
    context_from_events(events).messages().iter().filter(|message| !message.is_system()).cloned().collect()
}
