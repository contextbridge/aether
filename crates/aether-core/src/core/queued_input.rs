use llm::{ContentBlock, MessageId};

use crate::events::TaskOutcome;

#[derive(Debug)]
pub(super) enum QueuedInput {
    User { message_id: MessageId, content: Vec<ContentBlock> },
    TaskOutcome(Box<TaskOutcome>),
}

impl QueuedInput {
    pub(super) fn content_blocks(&self) -> Vec<ContentBlock> {
        match self {
            Self::User { content, .. } => content.clone(),
            Self::TaskOutcome(outcome) => outcome.content_blocks(),
        }
    }
}
