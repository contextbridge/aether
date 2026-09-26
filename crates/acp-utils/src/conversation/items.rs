use crate::content::{display_content_blocks, map_content_blocks_to_text};
use agent_client_protocol::schema::v2 as acp;
use serde::Serialize;
use std::borrow::Cow;
use std::sync::atomic::{AtomicU64, Ordering};

use super::tool_calls::ToolCall;

/// Identifies one generation of a [`super::Conversation`]'s items; a clear starts a new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct ConversationId(u64);

/// Identifies an item within one [`ConversationId`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct ConversationItemId(pub(super) u64);

/// Advances every time an item changes, so hosts can cache whatever they derive from it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct Revision(u64);

impl Revision {
    pub fn value(self) -> u64 {
        self.0
    }
}

/// Whether an item can still change. Items are sealed once their turn ends or,
/// for a tool call, once it and its sub-agents reach a terminal status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemState {
    Open,
    Sealed,
}

/// What an item holds. Messages keep the agent's content blocks, with adjacent
/// streamed text merged into one block.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", content = "content", rename_all = "snake_case")]
pub enum ConversationContent {
    User(Vec<acp::ContentBlock>),
    Assistant(Vec<acp::ContentBlock>),
    Tool(ToolCall),
    /// Information from the host rather than the agent.
    Notice(String),
}

#[derive(Debug, Clone, PartialEq, Serialize)]
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

    /// Advances when a change rewrites content rather than appending to it, which
    /// can invalidate anything a host already derived from the earlier content.
    pub fn replacement_revision(&self) -> Revision {
        self.replacement_revision
    }

    pub fn state(&self) -> ItemState {
        self.state
    }

    pub fn content(&self) -> &ConversationContent {
        &self.content
    }

    /// The item as plain text; `None` for a tool call.
    ///
    /// Media become placeholders, and a user message's embedded resources become
    /// references rather than their contents. Streamed text only ever grows at the
    /// end until the item's [`ConversationItem::replacement_revision`] advances.
    pub fn text(&self) -> Option<Cow<'_, str>> {
        match &self.content {
            ConversationContent::User(blocks) => Some(match blocks.as_slice() {
                [] | [acp::ContentBlock::Text(_)] => plain_text(blocks),
                blocks => Cow::Owned(map_content_blocks_to_text(display_content_blocks(blocks))),
            }),
            ConversationContent::Assistant(blocks) => Some(plain_text(blocks)),
            ConversationContent::Notice(text) => Some(Cow::Borrowed(text)),
            ConversationContent::Tool(_) => None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.state == ItemState::Open
    }

    pub(super) fn new(id: ConversationItemId, state: ItemState, content: ConversationContent) -> Self {
        Self {
            id,
            message_id: None,
            preserve_user_display: false,
            revision: Revision::default(),
            replacement_revision: Revision::default(),
            state,
            content,
        }
    }

    pub(super) fn changed(&mut self, replaces_content: bool) {
        if replaces_content {
            self.replacement_revision.bump();
        }
        self.revision.bump();
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
}

impl MessageRole {
    pub(super) fn content(self, blocks: Vec<acp::ContentBlock>) -> ConversationContent {
        match self {
            Self::User => ConversationContent::User(blocks),
            Self::Assistant => ConversationContent::Assistant(blocks),
        }
    }
}

/// Append a streamed block, merging text into a trailing text block. Returns whether anything changed.
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
    fn bump(&mut self) {
        self.0 = self.0.saturating_add(1);
    }
}

fn plain_text(blocks: &[acp::ContentBlock]) -> Cow<'_, str> {
    match blocks {
        [] => Cow::Borrowed(""),
        [acp::ContentBlock::Text(text)] => Cow::Borrowed(&text.text),
        blocks => Cow::Owned(map_content_blocks_to_text(blocks.to_vec())),
    }
}
