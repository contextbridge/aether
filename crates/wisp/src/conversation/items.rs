use super::TurnState;
use super::plan_tracker::PlanTracker;
use super::progress_indicator::ProgressIndicator;
use super::tool_calls::{ToolCall, ToolStatus};
use acp_utils::content::map_content_blocks_to_text;
use acp_utils::notifications::SubAgentProgressParams;
use agent_client_protocol::schema::{MaybeUndefined, v2 as acp};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_CONVERSATION_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ConversationId(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ConversationItemId(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Revision(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemState {
    Open,
    Sealed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageRole {
    User,
    Assistant,
    Thought,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextItem {
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConversationContent {
    User(TextItem),
    Assistant(TextItem),
    Thought(TextItem),
    Tool(ToolCall),
    Notice(Notice),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConversationItem {
    id: ConversationItemId,
    message_id: Option<acp::MessageId>,
    revision: Revision,
    state: ItemState,
    content: ConversationContent,
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

    pub fn state(&self) -> ItemState {
        self.state
    }

    pub fn content(&self) -> &ConversationContent {
        &self.content
    }

    pub fn text(&self) -> Option<&str> {
        match &self.content {
            ConversationContent::User(item)
            | ConversationContent::Assistant(item)
            | ConversationContent::Thought(item) => Some(&item.text),
            ConversationContent::Notice(notice) => Some(&notice.text),
            ConversationContent::Tool(_) => None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.state == ItemState::Open
    }
}

#[derive(Debug)]
pub struct Conversation {
    id: ConversationId,
    items: Vec<ConversationItem>,
    tool_index: HashMap<String, usize>,
    message_index: HashMap<acp::MessageId, usize>,
    pending_user: Option<usize>,
    next_item_id: u64,
    turn: TurnState,
    plan_tracker: PlanTracker,
    progress_indicator: ProgressIndicator,
}

impl Default for Conversation {
    fn default() -> Self {
        Self::new()
    }
}

impl Conversation {
    pub fn new() -> Self {
        Self {
            id: ConversationId(NEXT_CONVERSATION_ID.fetch_add(1, Ordering::Relaxed)),
            items: Vec::new(),
            tool_index: HashMap::new(),
            message_index: HashMap::new(),
            pending_user: None,
            next_item_id: 0,
            turn: TurnState::default(),
            plan_tracker: PlanTracker::default(),
            progress_indicator: ProgressIndicator::default(),
        }
    }

    pub fn id(&self) -> ConversationId {
        self.id
    }

    pub fn items(&self) -> &[ConversationItem] {
        &self.items
    }

    pub fn append_user_content(&mut self, text: impl Into<String>) -> ConversationItemId {
        self.seal_open_assistant();
        self.push(ItemState::Sealed, ConversationContent::User(TextItem { text: text.into() }))
    }

    pub fn append_pending_user_content(&mut self, text: impl Into<String>) -> ConversationItemId {
        self.seal_open_assistant();
        let id = self.push(ItemState::Open, ConversationContent::User(TextItem { text: text.into() }));
        self.pending_user = Some(self.items.len() - 1);
        id
    }

    pub fn upsert_message(
        &mut self,
        role: MessageRole,
        message_id: acp::MessageId,
        content: &MaybeUndefined<Vec<acp::ContentBlock>>,
    ) {
        let index = self.message_slot(role, message_id);
        let text = match content {
            MaybeUndefined::Undefined => return,
            MaybeUndefined::Null => String::new(),
            MaybeUndefined::Value(blocks) => map_content_blocks_to_text(blocks.clone()),
        };
        self.items[index].content = message_content(role, text);
        self.items[index].revision.bump();
    }

    pub fn append_message_chunk(&mut self, role: MessageRole, chunk: &acp::ContentChunk) {
        let index = self.message_slot(role, chunk.message_id.clone());
        let item = &mut self.items[index];
        let text = map_content_blocks_to_text(vec![chunk.content.clone()]);
        match &mut item.content {
            ConversationContent::User(current)
            | ConversationContent::Assistant(current)
            | ConversationContent::Thought(current) => current.text.push_str(&text),
            _ => return,
        }
        item.revision.bump();
    }

    pub fn append_notice(&mut self, text: impl Into<String>) -> ConversationItemId {
        self.seal_open_assistant();
        self.push(ItemState::Sealed, ConversationContent::Notice(Notice { text: text.into() }))
    }

    pub fn append_assistant_chunk(&mut self, chunk: &str) {
        if chunk.is_empty() {
            return;
        }
        let has_open_assistant = self.items.last().is_some_and(|item| {
            item.state == ItemState::Open
                && item.message_id.is_none()
                && matches!(item.content, ConversationContent::Assistant(_))
        });
        if !has_open_assistant {
            self.push(ItemState::Open, ConversationContent::Assistant(TextItem { text: String::new() }));
        }
        if let Some(item) = self.items.last_mut()
            && let ConversationContent::Assistant(text) = &mut item.content
        {
            text.text.push_str(chunk);
            item.revision.bump();
        }
    }

    pub fn finish_current_block(&mut self) {
        self.seal_open_assistant();
    }

    pub fn on_tool_call_update(&mut self, update: &acp::ToolCallUpdate) {
        let index = self.tool_slot(&update.tool_call_id);
        self.update_tool(index, |tool_call| tool_call.apply_update(update));
    }

    pub fn on_tool_call_content_chunk(&mut self, chunk: &acp::ToolCallContentChunk) {
        let index = self.tool_slot(&chunk.tool_call_id);
        self.update_tool(index, |tool_call| tool_call.append_content(chunk.content.clone()));
    }

    pub fn on_sub_agent_progress(&mut self, notification: &SubAgentProgressParams) {
        let Some(&index) = self.tool_index.get(&notification.parent_tool_id) else {
            return;
        };
        self.update_tool(index, |tool_call| tool_call.apply_sub_agent_progress(notification));
    }

    pub fn finish_turn(&mut self, terminal_status: &ToolStatus) {
        self.pending_user = None;
        for item in &mut self.items {
            if item.state != ItemState::Open {
                continue;
            }
            if let ConversationContent::Tool(tool_call) = &mut item.content {
                tool_call.finalize(terminal_status);
            }
            item.state = ItemState::Sealed;
            item.revision.bump();
        }
    }

    pub fn clear(&mut self) {
        self.id = ConversationId(NEXT_CONVERSATION_ID.fetch_add(1, Ordering::Relaxed));
        self.items.clear();
        self.tool_index.clear();
        self.message_index.clear();
        self.pending_user = None;
        self.next_item_id = 0;
    }

    pub fn turn(&self) -> &TurnState {
        &self.turn
    }

    pub fn turn_mut(&mut self) -> &mut TurnState {
        &mut self.turn
    }

    pub fn plan_tracker(&self) -> &PlanTracker {
        &self.plan_tracker
    }

    pub fn plan_tracker_mut(&mut self) -> &mut PlanTracker {
        &mut self.plan_tracker
    }

    pub fn progress_indicator(&self) -> &ProgressIndicator {
        &self.progress_indicator
    }

    pub fn progress_indicator_mut(&mut self) -> &mut ProgressIndicator {
        &mut self.progress_indicator
    }

    pub fn reset_feature_state(&mut self) {
        self.turn.reset();
        self.plan_tracker.clear();
        self.progress_indicator = ProgressIndicator::default();
    }

    pub fn any_running(&self) -> bool {
        self.items.iter().any(|item| match &item.content {
            ConversationContent::Tool(tool_call) => tool_call.is_running(),
            _ => false,
        })
    }

    fn message_slot(&mut self, role: MessageRole, message_id: acp::MessageId) -> usize {
        if let Some(&index) = self.message_index.get(&message_id) {
            return index;
        }
        let index = if role == MessageRole::User { self.pending_user.take() } else { None }.unwrap_or_else(|| {
            let index = self.items.len();
            self.push(ItemState::Open, message_content(role, String::new()));
            index
        });
        self.items[index].message_id = Some(message_id.clone());
        self.message_index.insert(message_id, index);
        index
    }

    fn tool_slot(&mut self, id: &acp::ToolCallId) -> usize {
        if let Some(&index) = self.tool_index.get(id.0.as_ref()) {
            return index;
        }
        let index = self.items.len();
        self.push(
            ItemState::Open,
            ConversationContent::Tool(ToolCall::from_update(&acp::ToolCallUpdate::new(id.clone()))),
        );
        self.tool_index.insert(id.to_string(), index);
        index
    }

    fn push(&mut self, state: ItemState, content: ConversationContent) -> ConversationItemId {
        let id = ConversationItemId(self.next_item_id);
        self.next_item_id = self.next_item_id.saturating_add(1);
        self.items.push(ConversationItem { id, message_id: None, revision: Revision(0), state, content });
        id
    }

    fn seal_open_assistant(&mut self) {
        if let Some(item) = self.items.last_mut()
            && item.state == ItemState::Open
            && item.message_id.is_none()
            && matches!(item.content, ConversationContent::Assistant(_))
        {
            item.state = ItemState::Sealed;
            item.revision.bump();
        }
    }

    fn update_tool(&mut self, index: usize, apply: impl FnOnce(&mut ToolCall)) {
        let item = &mut self.items[index];
        if let ConversationContent::Tool(tool_call) = &mut item.content {
            apply(tool_call);
            item.state = if tool_call.rendering_final() { ItemState::Sealed } else { ItemState::Open };
            item.revision.bump();
        }
    }
}

fn message_content(role: MessageRole, text: String) -> ConversationContent {
    match role {
        MessageRole::User => ConversationContent::User(TextItem { text }),
        MessageRole::Assistant => ConversationContent::Assistant(TextItem { text }),
        MessageRole::Thought => ConversationContent::Thought(TextItem { text }),
    }
}

impl Revision {
    fn bump(&mut self) {
        self.0 = self.0.saturating_add(1);
    }
}
