use super::actor::ClientConnection;
use crate::acp::protocol::content::map_user_message;
use aether_sessions::model::{SessionEvent, UserEvent};
use agent_client_protocol::Error;
use agent_client_protocol::schema::v2 as acp;
use futures::future::BoxFuture;
use futures::stream::FuturesOrdered;
use futures::{FutureExt, StreamExt};
use llm::{ContentBlock, MessageId};
use std::collections::HashMap;

#[derive(Default)]
pub(crate) struct PromptQueue {
    running: bool,
    expansions: FuturesOrdered<BoxFuture<'static, (MessageId, Vec<ContentBlock>)>>,
    preparing: HashMap<MessageId, PreparingPrompt>,
    submitted: HashMap<MessageId, SubmittedPrompt>,
}

pub(crate) struct SubmittedPrompt {
    pub responder: ClientConnection,
    pub event: SessionEvent,
    pub echo: acp::UserMessage,
}

struct PreparingPrompt {
    responder: ClientConnection,
    display_content: Vec<ContentBlock>,
}

impl PromptQueue {
    pub(crate) fn is_idle(&self) -> bool {
        !self.running && self.preparing.is_empty() && self.submitted.is_empty()
    }

    pub(crate) fn is_running(&self) -> bool {
        self.running
    }

    pub(crate) fn is_preparing(&self) -> bool {
        !self.expansions.is_empty()
    }

    pub(crate) fn has_agent_work(&self) -> bool {
        self.running || !self.submitted.is_empty()
    }

    pub(crate) fn turn_started(&mut self) {
        self.running = true;
    }

    pub(crate) fn turn_ended(&mut self) {
        self.running = false;
    }

    pub(crate) fn prepare(
        &mut self,
        display_content: Vec<ContentBlock>,
        responder: ClientConnection,
        expansion: BoxFuture<'static, Vec<ContentBlock>>,
    ) {
        let message_id = MessageId::new();
        self.preparing.insert(message_id.clone(), PreparingPrompt { responder, display_content });
        self.expansions.push_back(expansion.map(move |content| (message_id, content)).boxed());
    }

    pub(crate) async fn next_prepared(&mut self) -> Option<(MessageId, Vec<ContentBlock>)> {
        self.expansions.next().await
    }

    pub(crate) fn submit(&mut self, message_id: MessageId, content: Vec<ContentBlock>) {
        let Some(PreparingPrompt { responder, display_content }) = self.preparing.remove(&message_id) else {
            return;
        };
        let echo = map_user_message(message_id.to_string().into(), &display_content);
        let event = SessionEvent::User(UserEvent::Message {
            message_id: message_id.clone(),
            display_content: (display_content != content).then_some(display_content),
            content,
        });
        self.submitted.insert(message_id, SubmittedPrompt { responder, event, echo });
    }

    pub(crate) fn reject(&mut self, message_id: &MessageId, error: Error) {
        if let Some(prompt) = self.preparing.remove(message_id) {
            prompt.responder.respond_with_error(error);
        }
    }

    pub(crate) fn take_submitted(&mut self, message_id: &MessageId) -> Option<SubmittedPrompt> {
        self.submitted.remove(message_id)
    }

    pub(crate) fn cancel_preparing(&mut self) {
        self.expansions = FuturesOrdered::new();
        for (_, prompt) in self.preparing.drain() {
            prompt.responder.respond_with_error(Error::request_cancelled());
        }
    }

    pub(crate) fn cancel_submitted(&mut self) {
        for (_, prompt) in self.submitted.drain() {
            prompt.responder.respond_with_error(Error::request_cancelled());
        }
    }
}
