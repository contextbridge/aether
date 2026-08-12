use crate::events::TaskOutcome;

#[derive(Debug)]
pub(super) enum QueuedInput {
    User(Vec<llm::ContentBlock>),
    TaskOutcome(Box<TaskOutcome>),
}

impl QueuedInput {
    pub(super) fn content_blocks(&self) -> Vec<llm::ContentBlock> {
        match self {
            Self::User(content) => content.clone(),
            Self::TaskOutcome(outcome) => outcome.content_blocks(),
        }
    }
}
