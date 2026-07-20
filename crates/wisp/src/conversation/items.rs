use super::TurnState;
use super::plan_tracker::PlanTracker;
use super::progress_indicator::ProgressIndicator;
use super::tool_calls::{ToolCall, ToolStatus, raw_input_fragment};
use crate::view::markdown::{FenceLine, complete_lines_with_fences};
use acp_utils::notifications::SubAgentProgressParams;
use agent_client_protocol::schema::v1 as acp;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_CONVERSATION_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ConversationId(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ConversationItemId(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Revision(u64);

/// `Sealed` marks an item whose rendering is final: it may enter the
/// terminal's native scrollback, which can never be rewritten. An `Open` item
/// may still redraw in place and must stay in the live viewport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemState {
    Open,
    Sealed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextItem {
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversationContent {
    User(TextItem),
    Assistant(TextItem),
    Tool(ToolCall),
    Notice(Notice),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationItem {
    id: ConversationItemId,
    revision: Revision,
    state: ItemState,
    content: ConversationContent,
}

impl ConversationItem {
    pub fn id(&self) -> ConversationItemId {
        self.id
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
            ConversationContent::User(item) | ConversationContent::Assistant(item) => Some(&item.text),
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

    pub fn append_notice(&mut self, text: impl Into<String>) -> ConversationItemId {
        self.seal_open_assistant();
        self.push(ItemState::Sealed, ConversationContent::Notice(Notice { text: text.into() }))
    }

    pub fn append_assistant_chunk(&mut self, chunk: &str) {
        if chunk.is_empty() {
            return;
        }
        let has_open_assistant = self.items.last().is_some_and(|item| {
            item.state == ItemState::Open && matches!(item.content, ConversationContent::Assistant(_))
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
        while let Some(finalized_end) = self.items.last().and_then(|item| match &item.content {
            ConversationContent::Assistant(text) => complete_lines_with_fences(&text.text)
                .find(|(_, line)| matches!(line, FenceLine::Blank))
                .map(|(offset, _)| offset),
            _ => None,
        }) {
            let trailing = match self.items.last_mut() {
                Some(ConversationItem { content: ConversationContent::Assistant(text), state, revision, .. }) => {
                    let trailing = text.text.split_off(finalized_end);
                    *state = ItemState::Sealed;
                    revision.bump();
                    trailing
                }
                _ => break,
            };
            if trailing.is_empty() {
                break;
            }
            self.push(ItemState::Open, ConversationContent::Assistant(TextItem { text: trailing }));
        }
    }

    pub fn finish_current_block(&mut self) {
        self.seal_open_assistant();
    }

    pub fn on_tool_call(&mut self, tool_call: &acp::ToolCall) {
        self.seal_open_assistant();
        let id = tool_call.tool_call_id.0.to_string();
        if let Some(&index) = self.tool_index.get(&id) {
            if self.items[index].state == ItemState::Open
                && let ConversationContent::Tool(current) = &mut self.items[index].content
            {
                if !tool_call.title.is_empty() {
                    current.title.clone_from(&tool_call.title);
                }
                current.status = ToolStatus::Running;
                current.raw_input = tool_call.raw_input.as_ref().map_or_else(String::new, raw_input_fragment);
                self.items[index].revision.bump();
            }
            return;
        }
        let index = self.items.len();
        self.tool_index.insert(id, index);
        let item_id = self.next_id();
        self.items.push(ConversationItem {
            id: item_id,
            revision: Revision(0),
            state: ItemState::Open,
            content: ConversationContent::Tool(ToolCall::from_acp(tool_call)),
        });
    }

    pub fn on_tool_call_update(&mut self, update: &acp::ToolCallUpdate) {
        let Some(&index) = self.tool_index.get(update.tool_call_id.0.as_ref()) else {
            return;
        };
        self.update_open_tool(index, |tool_call| tool_call.apply_update(update));
    }

    pub fn on_sub_agent_progress(&mut self, notification: &SubAgentProgressParams) {
        let Some(&index) = self.tool_index.get(&notification.parent_tool_id) else {
            return;
        };
        self.update_open_tool(index, |tool_call| tool_call.apply_sub_agent_progress(notification));
    }

    pub fn finish_turn(&mut self, terminal_status: &ToolStatus) {
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

    fn push(&mut self, state: ItemState, content: ConversationContent) -> ConversationItemId {
        let id = self.next_id();
        self.items.push(ConversationItem { id, revision: Revision(0), state, content });
        id
    }

    fn next_id(&mut self) -> ConversationItemId {
        let id = ConversationItemId(self.next_item_id);
        self.next_item_id = self.next_item_id.saturating_add(1);
        id
    }

    fn seal_open_assistant(&mut self) {
        if let Some(item) = self.items.last_mut()
            && item.state == ItemState::Open
            && matches!(item.content, ConversationContent::Assistant(_))
        {
            item.state = ItemState::Sealed;
            item.revision.bump();
        }
    }

    fn update_open_tool(&mut self, index: usize, apply: impl FnOnce(&mut ToolCall)) {
        let item = &mut self.items[index];
        if item.state == ItemState::Open
            && let ConversationContent::Tool(tool_call) = &mut item.content
        {
            apply(tool_call);
            if tool_call.rendering_final() {
                item.state = ItemState::Sealed;
            }
            item.revision.bump();
        }
    }
}

impl Revision {
    fn bump(&mut self) {
        self.0 = self.0.saturating_add(1);
    }
}
