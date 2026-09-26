use crate::content::{display_content_blocks, map_content_blocks_to_text};
use agent_client_protocol::schema::v2 as acp;
use schemars::JsonSchema;
use serde::Serialize;
use std::borrow::Cow;
use std::sync::atomic::{AtomicU64, Ordering};

use super::tool_calls::ToolCall;

/// Id for one generation of a [`super::Conversation`]'s items.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct ConversationId(u64);

/// Id for an item within one [`ConversationId`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, JsonSchema)]
pub struct ConversationItemId(pub(super) u64);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, JsonSchema)]
pub struct Revision(u64);

impl Revision {
    pub fn value(self) -> u64 {
        self.0
    }
}

/// Whether an item can still change. Items are sealed once their turn ends or,
/// for a tool call, once it and its sub-agents reach a terminal status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ItemState {
    Open,
    Sealed,
}

/// Content an item holds
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(tag = "kind", content = "content", rename_all = "snake_case")]
pub enum ConversationContent {
    User(Vec<acp::ContentBlock>),
    Assistant(Vec<acp::ContentBlock>),
    Thought(Vec<acp::ContentBlock>),
    Tool(ToolCall),
    /// Information from the host rather than the agent.
    Notice(String),
}

/// An item keeps its `id` for the life of the conversation; `revision` advances whenever it changes.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConversationItem {
    id: ConversationItemId,
    pub(super) message_id: Option<acp::MessageId>,
    #[serde(skip)]
    pub(super) preserve_user_display: bool,
    revision: Revision,
    #[serde(skip)]
    replacement_revision: Revision,
    pub(super) state: ItemState,
    #[serde(flatten)]
    pub(super) content: ConversationContent,
}

impl ConversationItem {
    pub fn id(&self) -> ConversationItemId {
        self.id
    }

    pub fn message_id(&self) -> Option<&acp::MessageId> {
        self.message_id.as_ref()
    }

    pub fn revision(&self) -> Revision {
        self.revision
    }

    pub fn replacement_revision(&self) -> Revision {
        self.replacement_revision
    }

    pub fn state(&self) -> ItemState {
        self.state
    }

    pub fn content(&self) -> &ConversationContent {
        &self.content
    }

    pub fn text(&self) -> Option<Cow<'_, str>> {
        match &self.content {
            ConversationContent::User(blocks) => Some(match blocks.as_slice() {
                [] | [acp::ContentBlock::Text(_)] => plain_text(blocks),
                blocks => Cow::Owned(map_content_blocks_to_text(&display_content_blocks(blocks))),
            }),
            ConversationContent::Assistant(blocks) | ConversationContent::Thought(blocks) => Some(plain_text(blocks)),
            ConversationContent::Notice(text) => Some(Cow::Borrowed(text)),
            ConversationContent::Tool(_) => None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.state == ItemState::Open
    }

    pub(super) fn new(
        id: ConversationItemId,
        revision: Revision,
        state: ItemState,
        content: ConversationContent,
    ) -> Self {
        Self {
            id,
            message_id: None,
            preserve_user_display: false,
            revision,
            replacement_revision: revision,
            state,
            content,
        }
    }

    pub(super) fn touch(&mut self, revision: Revision, rewrites: bool) {
        if rewrites {
            self.replacement_revision = revision;
        }
        self.revision = revision;
    }
}

impl ConversationId {
    pub(super) fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MessageRole {
    User,
    Assistant,
    Thought,
}

impl MessageRole {
    pub(super) fn content(self, blocks: Vec<acp::ContentBlock>) -> ConversationContent {
        match self {
            Self::User => ConversationContent::User(blocks),
            Self::Assistant => ConversationContent::Assistant(blocks),
            Self::Thought => ConversationContent::Thought(blocks),
        }
    }

    pub(super) fn blocks_mut(self, content: &mut ConversationContent) -> Option<&mut Vec<acp::ContentBlock>> {
        match (self, content) {
            (Self::User, ConversationContent::User(blocks))
            | (Self::Assistant, ConversationContent::Assistant(blocks))
            | (Self::Thought, ConversationContent::Thought(blocks)) => Some(blocks),
            _ => None,
        }
    }
}

pub(super) fn append_block(blocks: &mut Vec<acp::ContentBlock>, block: &acp::ContentBlock) -> bool {
    match (blocks.last_mut(), block) {
        (_, acp::ContentBlock::Text(text)) if text.text.is_empty() => false,
        (Some(acp::ContentBlock::Text(last)), acp::ContentBlock::Text(text)) => {
            last.text.push_str(&text.text);
            true
        }
        _ => {
            blocks.push(block.clone());
            true
        }
    }
}

impl Revision {
    pub(super) fn advance(&mut self) -> Self {
        self.0 = self.0.saturating_add(1);
        *self
    }
}

fn plain_text(blocks: &[acp::ContentBlock]) -> Cow<'_, str> {
    match blocks {
        [] => Cow::Borrowed(""),
        [acp::ContentBlock::Text(text)] => Cow::Borrowed(&text.text),
        blocks => Cow::Owned(map_content_blocks_to_text(blocks)),
    }
}
