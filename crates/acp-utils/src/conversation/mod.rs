//! A host-independent model of one ACP session's conversation.
//!
//! [`Conversation`] reduces `session/update` notifications, Aether sub-agent
//! progress and the host's own prompt lifecycle into ordered items plus the
//! state of the current turn. Hosts route events to the right conversation
//! (by session id) and draw it; the reduction rules live here so every UI
//! built on ACP agrees on them.

mod activity;
mod items;
mod tool_calls;
mod turn;

pub use activity::{Activity, ActivityPhase};
pub use items::{ConversationContent, ConversationId, ConversationItem, ConversationItemId, ItemState, Revision};
pub use tool_calls::{SubAgentState, SubAgentToolCall, ToolCall, ToolStatus};
pub use turn::{TurnFinished, TurnPhase};

use crate::notifications::SubAgentProgressParams;
use agent_client_protocol::schema::{MaybeUndefined, v2 as acp};
use items::MessageRole;
use std::collections::{HashMap, HashSet};

/// One session's conversation: its items and the state of the current turn.
#[derive(Debug)]
pub struct Conversation {
    id: ConversationId,
    items: Vec<ConversationItem>,
    tool_index: HashMap<String, usize>,
    message_index: HashMap<acp::MessageId, usize>,
    pending_user: Option<usize>,
    next_item_id: u64,
    turn: TurnPhase,
    activity: Activity,
    compactions: HashSet<acp::CompactionId>,
    context_usage: Option<acp::UsageUpdate>,
    plan: Option<acp::PlanItems>,
}

impl Default for Conversation {
    fn default() -> Self {
        Self::new()
    }
}

impl Conversation {
    pub fn new() -> Self {
        Self {
            id: ConversationId::next(),
            items: Vec::new(),
            tool_index: HashMap::new(),
            message_index: HashMap::new(),
            pending_user: None,
            next_item_id: 0,
            turn: TurnPhase::Idle,
            activity: Activity::default(),
            compactions: HashSet::new(),
            context_usage: None,
            plan: None,
        }
    }

    /// Reduce one `session/update` for this conversation's session.
    ///
    /// Returns [`TurnFinished`] when the update ends a turn the conversation was
    /// waiting on. A `running` state while idle adopts a turn started elsewhere,
    /// such as the live turn of a session this client just attached to.
    pub fn apply_update(&mut self, update: &acp::SessionUpdate) -> Option<TurnFinished> {
        if matches!(update, acp::SessionUpdate::StateUpdate(acp::StateUpdate::Running(_))) && self.turn.is_idle() {
            self.turn = TurnPhase::Running;
            self.activity.prompt_started();
        }
        if self.turn.waiting_for_response() {
            self.activity.observe(update);
        }
        match update {
            acp::SessionUpdate::CompactionUpdate(update) => {
                if self.activity.accepting() {
                    self.apply_compaction(update);
                }
            }
            acp::SessionUpdate::StateUpdate(acp::StateUpdate::Idle(idle)) if self.turn.waiting_for_response() => {
                return Some(self.finish_turn(idle.stop_reason.clone()));
            }
            acp::SessionUpdate::UserMessage(message) => {
                self.upsert_message(MessageRole::User, message.message_id.clone(), &message.content);
            }
            acp::SessionUpdate::AgentMessage(message) => {
                self.upsert_message(MessageRole::Assistant, message.message_id.clone(), &message.content);
            }
            acp::SessionUpdate::UserMessageChunk(chunk) => self.append_message_chunk(MessageRole::User, chunk),
            acp::SessionUpdate::AgentMessageChunk(chunk) => self.append_message_chunk(MessageRole::Assistant, chunk),
            acp::SessionUpdate::ToolCallUpdate(update) => {
                let index = self.tool_slot(&update.tool_call_id);
                self.update_tool(index, |tool_call| tool_call.apply_update(update));
            }
            acp::SessionUpdate::ToolCallContentChunk(chunk) => {
                let index = self.tool_slot(&chunk.tool_call_id);
                self.update_tool(index, |tool_call| tool_call.append_content(chunk.content.clone()));
            }
            acp::SessionUpdate::PlanUpdate(update) => {
                if let acp::PlanUpdateContent::Items(items) = &update.plan {
                    self.plan = Some(items.clone());
                }
            }
            acp::SessionUpdate::UsageUpdate(usage) => self.context_usage = Some(usage.clone()),
            _ => {}
        }
        None
    }

    /// Reduce an Aether sub-agent progress notification into the tool call that spawned it.
    pub fn apply_sub_agent_progress(&mut self, progress: &SubAgentProgressParams) {
        if !self.activity.accepting() {
            return;
        }
        let Some(&index) = self.tool_index.get(&progress.parent_tool_id) else {
            return;
        };
        self.update_tool(index, |tool_call| tool_call.apply_sub_agent_progress(progress));
    }

    /// The host sent `session/prompt`; the turn is owed until the agent accepts it and reaches idle.
    pub fn start_prompt(&mut self) {
        self.turn = TurnPhase::Submitting;
        self.activity.prompt_started();
    }

    /// The agent answered `session/prompt` successfully.
    pub fn accept_prompt(&mut self) {
        self.turn.accept();
    }

    /// The agent answered `session/prompt` with an error; any turn still in flight ends as failed.
    pub fn reject_prompt(&mut self, error: &str) {
        if self.turn.waiting_for_response() {
            self.end_turn(&ToolStatus::Error(format!("failed: {error}")));
        }
        self.turn = TurnPhase::Idle;
    }

    /// The connection is gone. The agent may still be running, but nothing more will arrive here.
    pub fn connection_closed(&mut self) {
        self.turn = TurnPhase::Idle;
        self.activity.prompt_finished();
    }

    /// Drop all items and turn state under a new [`ConversationId`], as on a context clear or a
    /// session switch. A prompt whose response is still owed stays owed.
    pub fn clear(&mut self) {
        self.turn.finish();
        *self = Self { turn: self.turn, ..Self::new() };
    }

    pub fn append_user_content(&mut self, content: Vec<acp::ContentBlock>) -> ConversationItemId {
        self.push(ItemState::Sealed, ConversationContent::User(content))
    }

    /// Echo a submitted prompt before the agent acknowledges it; the next
    /// `user_message` update adopts this item instead of appending another, and
    /// the echo keeps the content given here.
    pub fn append_pending_user_content(&mut self, content: Vec<acp::ContentBlock>) -> ConversationItemId {
        let id = self.push(ItemState::Open, ConversationContent::User(content));
        let index = self.items.len() - 1;
        self.items[index].preserve_user_display = true;
        self.pending_user = Some(index);
        id
    }

    /// Add information from the host itself rather than the agent.
    pub fn append_notice(&mut self, text: impl Into<String>) -> ConversationItemId {
        self.push(ItemState::Sealed, ConversationContent::Notice(text.into()))
    }

    /// Changes whenever [`Conversation::clear`] replaces the items.
    pub fn id(&self) -> ConversationId {
        self.id
    }

    pub fn items(&self) -> &[ConversationItem] {
        &self.items
    }

    pub fn turn(&self) -> TurnPhase {
        self.turn
    }

    pub fn activity(&self) -> &Activity {
        &self.activity
    }

    pub fn plan(&self) -> Option<&acp::PlanItems> {
        self.plan.as_ref()
    }

    pub fn context_usage(&self) -> Option<&acp::UsageUpdate> {
        self.context_usage.as_ref()
    }

    pub fn is_compacting(&self) -> bool {
        !self.compactions.is_empty()
    }

    /// A prompt is outstanding, so the agent owes a reply.
    pub fn waiting_for_response(&self) -> bool {
        self.turn.waiting_for_response()
    }

    /// Some tool call, or a sub-agent under one, has not reached a terminal status.
    pub fn any_running(&self) -> bool {
        self.items.iter().any(|item| match item.content() {
            ConversationContent::Tool(tool_call) => tool_call.is_running(),
            _ => false,
        })
    }

    fn finish_turn(&mut self, stop_reason: Option<acp::StopReason>) -> TurnFinished {
        let status = match stop_reason {
            Some(acp::StopReason::Cancelled) => ToolStatus::Error("cancelled".to_string()),
            _ => ToolStatus::Success,
        };
        self.end_turn(&status);
        TurnFinished { stop_reason }
    }

    fn end_turn(&mut self, terminal_status: &ToolStatus) {
        self.turn.finish();
        self.compactions.clear();
        self.activity.prompt_finished();
        self.pending_user = None;
        for item in &mut self.items {
            if item.state == ItemState::Open {
                if let ConversationContent::Tool(tool_call) = &mut item.content {
                    tool_call.finalize(terminal_status);
                }
                item.state = ItemState::Sealed;
                item.changed(false);
            }
        }
    }

    fn apply_compaction(&mut self, update: &acp::CompactionUpdate) {
        match update.status {
            acp::CompactionStatus::InProgress => {
                self.compactions.insert(update.compaction_id.clone());
            }
            acp::CompactionStatus::Completed | acp::CompactionStatus::Failed | acp::CompactionStatus::Cancelled => {
                self.compactions.remove(&update.compaction_id);
            }
            _ => {}
        }
    }

    fn upsert_message(
        &mut self,
        role: MessageRole,
        message_id: acp::MessageId,
        content: &MaybeUndefined<Vec<acp::ContentBlock>>,
    ) {
        let index = self.message_slot(role, message_id);
        let item = &mut self.items[index];
        if item.preserve_user_display {
            return;
        }
        let blocks = match content {
            MaybeUndefined::Undefined => return,
            MaybeUndefined::Null => Vec::new(),
            MaybeUndefined::Value(blocks) => blocks.clone(),
        };
        let content = role.content(blocks);
        if item.content != content {
            item.content = content;
            item.changed(true);
        }
    }

    fn append_message_chunk(&mut self, role: MessageRole, chunk: &acp::ContentChunk) {
        let index = self.message_slot(role, chunk.message_id.clone());
        let item = &mut self.items[index];
        if item.preserve_user_display {
            return;
        }
        let (ConversationContent::User(blocks) | ConversationContent::Assistant(blocks)) = &mut item.content else {
            return;
        };
        if !items::append_block(blocks, &chunk.content) {
            return;
        }
        let replaces_history = !item.is_open() || role == MessageRole::User;
        item.changed(replaces_history);
    }

    fn message_slot(&mut self, role: MessageRole, message_id: acp::MessageId) -> usize {
        if let Some(&index) = self.message_index.get(&message_id) {
            return index;
        }
        let index = if role == MessageRole::User { self.pending_user.take() } else { None }.unwrap_or_else(|| {
            let index = self.items.len();
            self.push(ItemState::Open, role.content(Vec::new()));
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
        self.items.push(ConversationItem::new(id, state, content));
        id
    }

    fn update_tool(&mut self, index: usize, apply: impl FnOnce(&mut ToolCall)) {
        let item = &mut self.items[index];
        if let ConversationContent::Tool(tool_call) = &mut item.content {
            let previous = tool_call.clone();
            apply(tool_call);
            if *tool_call == previous {
                return;
            }
            let state = if tool_call.rendering_final() { ItemState::Sealed } else { ItemState::Open };
            item.changed(!item.is_open());
            item.state = state;
        }
    }
}
