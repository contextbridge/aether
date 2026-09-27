mod activity;
mod items;
mod tool_calls;
mod turn;

pub use activity::Activity;
pub use items::{ConversationContent, ConversationId, ConversationItem, ConversationItemId, ItemState, Revision};
pub use tool_calls::{SubAgentState, SubAgentToolCall, ToolCall, ToolStatus};
pub use turn::{TurnFinished, TurnInProgress, TurnPhase};

use crate::client::AcpEvent;
use crate::notifications::SubAgentProgressParams;
use agent_client_protocol::schema::{MaybeUndefined, v2 as acp};
use items::MessageRole;
use std::collections::{HashMap, HashSet};
use std::fmt::Display;

/// One session's conversation items and state of the current turn.
#[derive(Debug)]
pub struct Conversation {
    id: ConversationId,
    revision: Revision,
    items: Vec<ConversationItem>,
    tool_index: HashMap<String, usize>,
    message_index: HashMap<acp::MessageId, usize>,
    pending_user: Option<usize>,
    next_item_id: u64,
    turn: TurnPhase,
    turn_ended: bool,
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
            revision: Revision::default(),
            items: Vec::new(),
            tool_index: HashMap::new(),
            message_index: HashMap::new(),
            pending_user: None,
            next_item_id: 0,
            turn: TurnPhase::Idle,
            turn_ended: false,
            activity: Activity::Idle,
            compactions: HashSet::new(),
            context_usage: None,
            plan: None,
        }
    }

    pub fn apply_event(&mut self, event: &AcpEvent) -> Option<TurnFinished> {
        match event {
            AcpEvent::SessionUpdate(notification) => return self.apply_update(&notification.update),
            AcpEvent::SubAgentProgress(progress) => self.apply_sub_agent_progress(progress),
            AcpEvent::ContextCleared(_) => self.clear(),
            AcpEvent::ConnectionClosed => self.connection_closed(),
            AcpEvent::AuthMethodsUpdated(_)
            | AcpEvent::McpNotification(_)
            | AcpEvent::GitDiffEvent(_)
            | AcpEvent::ElicitationRequest { .. } => {}
        }
        None
    }

    pub fn start_prompt(&mut self, echo: Option<Vec<acp::ContentBlock>>) -> Result<(), TurnInProgress> {
        if !self.turn.is_idle() {
            return Err(TurnInProgress);
        }
        if let Some(content) = echo {
            self.push(ItemState::Open, ConversationContent::User(content));
            let index = self.items.len() - 1;
            self.items[index].preserve_user_display = true;
            self.pending_user = Some(index);
        }
        self.begin_turn(TurnPhase::Submitting);
        Ok(())
    }

    /// The agent accepted the prompt.
    pub fn accept_prompt(&mut self) {
        self.set_turn(self.turn.accepted());
    }

    pub fn fail_prompt(&mut self, reason: impl Display) {
        if self.turn.waiting_for_response() {
            self.end_turn(ToolStatus::Failed, Some(&reason.to_string()));
        }
        self.set_turn(TurnPhase::Idle);
    }

    pub fn clear(&mut self) {
        *self = Self { turn: self.turn.finished(), revision: self.revision, ..Self::new() };
        self.advance();
    }

    pub fn append_user_content(&mut self, content: Vec<acp::ContentBlock>) -> ConversationItemId {
        self.push(ItemState::Sealed, ConversationContent::User(content))
    }

    pub fn append_notice(&mut self, text: impl Into<String>) -> ConversationItemId {
        self.push(ItemState::Sealed, ConversationContent::Notice(text.into()))
    }

    pub fn id(&self) -> ConversationId {
        self.id
    }

    pub fn revision(&self) -> Revision {
        self.revision
    }

    pub fn items(&self) -> &[ConversationItem] {
        &self.items
    }

    pub fn turn(&self) -> TurnPhase {
        self.turn
    }

    pub fn activity(&self) -> Activity {
        self.activity
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

    pub fn any_running(&self) -> bool {
        self.items.iter().any(|item| match item.content() {
            ConversationContent::Tool(tool_call) => tool_call.is_running(),
            _ => false,
        })
    }

    fn apply_update(&mut self, update: &acp::SessionUpdate) -> Option<TurnFinished> {
        if matches!(update, acp::SessionUpdate::StateUpdate(acp::StateUpdate::Running(_))) && self.turn.is_idle() {
            self.begin_turn(TurnPhase::Running);
        }
        if self.turn.waiting_for_response()
            && let Some(activity) = Activity::after(update)
        {
            self.set_activity(activity);
        }
        match update {
            acp::SessionUpdate::CompactionUpdate(update) => {
                if !self.turn_ended {
                    self.apply_compaction(update);
                }
            }
            acp::SessionUpdate::StateUpdate(acp::StateUpdate::Idle(idle)) if self.turn.waiting_for_response() => {
                return Some(self.finish_turn(idle.stop_reason.clone()));
            }
            acp::SessionUpdate::UserMessage(message) => {
                self.upsert_message(MessageRole::User, &message.message_id, &message.content);
            }
            acp::SessionUpdate::AgentMessage(message) => {
                self.upsert_message(MessageRole::Assistant, &message.message_id, &message.content);
            }
            acp::SessionUpdate::AgentThought(message) => {
                self.upsert_message(MessageRole::Thought, &message.message_id, &message.content);
            }
            acp::SessionUpdate::UserMessageChunk(chunk) => self.append_message_chunk(MessageRole::User, chunk),
            acp::SessionUpdate::AgentMessageChunk(chunk) => self.append_message_chunk(MessageRole::Assistant, chunk),
            acp::SessionUpdate::AgentThoughtChunk(chunk) => self.append_message_chunk(MessageRole::Thought, chunk),
            acp::SessionUpdate::ToolCallUpdate(update) => {
                let index = self.tool_slot(&update.tool_call_id);
                self.update_tool(index, |tool_call| tool_call.apply_update(update));
            }
            acp::SessionUpdate::ToolCallContentChunk(chunk) => {
                let index = self.tool_slot(&chunk.tool_call_id);
                self.update_tool(index, |tool_call| tool_call.append_content(chunk.content.clone()));
            }
            acp::SessionUpdate::PlanUpdate(update) => {
                if let acp::PlanUpdateContent::Items(items) = &update.plan
                    && self.plan.as_ref() != Some(items)
                {
                    self.plan = Some(items.clone());
                    self.advance();
                }
            }
            acp::SessionUpdate::UsageUpdate(usage) if self.context_usage.as_ref() != Some(usage) => {
                self.context_usage = Some(usage.clone());
                self.advance();
            }
            _ => {}
        }
        None
    }

    fn apply_sub_agent_progress(&mut self, progress: &SubAgentProgressParams) {
        if self.turn_ended {
            return;
        }
        let Some(&index) = self.tool_index.get(&progress.parent_tool_id) else {
            return;
        };
        self.update_tool(index, |tool_call| tool_call.apply_sub_agent_progress(progress));
    }

    fn connection_closed(&mut self) {
        self.set_turn(TurnPhase::Idle);
        self.turn_ended = true;
        self.set_activity(Activity::Idle);
    }

    fn begin_turn(&mut self, phase: TurnPhase) {
        self.turn = phase;
        self.turn_ended = false;
        self.activity = Activity::Thinking;
        self.advance();
    }

    fn set_turn(&mut self, turn: TurnPhase) {
        if self.turn != turn {
            self.turn = turn;
            self.advance();
        }
    }

    fn set_activity(&mut self, activity: Activity) {
        if self.activity != activity {
            self.activity = activity;
            self.advance();
        }
    }

    fn finish_turn(&mut self, stop_reason: Option<acp::StopReason>) -> TurnFinished {
        let status = match stop_reason {
            Some(acp::StopReason::Cancelled) => ToolStatus::Cancelled,
            _ => ToolStatus::Success,
        };
        self.end_turn(status, None);
        TurnFinished { stop_reason }
    }

    fn end_turn(&mut self, status: ToolStatus, error: Option<&str>) {
        self.turn = self.turn.finished();
        self.turn_ended = true;
        self.compactions.clear();
        self.activity = Activity::Idle;
        self.pending_user = None;
        let revision = self.advance();
        for item in self.items.iter_mut().filter(|item| item.is_open()) {
            if let ConversationContent::Tool(tool_call) = &mut item.content {
                tool_call.finalize(status, error);
            }
            item.state = ItemState::Sealed;
            item.touch(revision, false);
        }
    }

    fn apply_compaction(&mut self, update: &acp::CompactionUpdate) {
        let changed = match update.status {
            acp::CompactionStatus::InProgress => self.compactions.insert(update.compaction_id.clone()),
            acp::CompactionStatus::Completed | acp::CompactionStatus::Failed | acp::CompactionStatus::Cancelled => {
                self.compactions.remove(&update.compaction_id)
            }
            _ => false,
        };
        if changed {
            self.advance();
        }
    }

    fn upsert_message(
        &mut self,
        role: MessageRole,
        message_id: &acp::MessageId,
        content: &MaybeUndefined<Vec<acp::ContentBlock>>,
    ) {
        let blocks = match content {
            MaybeUndefined::Value(blocks) => blocks.clone(),
            MaybeUndefined::Null if self.message_index.contains_key(message_id) => Vec::new(),
            MaybeUndefined::Null | MaybeUndefined::Undefined => return,
        };
        let index = self.message_slot(role, message_id.clone());
        let item = &mut self.items[index];
        if item.preserve_user_display {
            return;
        }
        let content = role.content(blocks);
        if item.content != content {
            item.content = content;
            self.touch(index, true);
        }
    }

    fn append_message_chunk(&mut self, role: MessageRole, chunk: &acp::ContentChunk) {
        let index = self.message_slot(role, chunk.message_id.clone());
        let item = &mut self.items[index];
        if item.preserve_user_display {
            return;
        }
        let rewrites = !item.is_open() || role == MessageRole::User;
        if role.blocks_mut(&mut item.content).is_some_and(|blocks| items::append_block(blocks, &chunk.content)) {
            self.touch(index, rewrites);
        }
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
        let revision = self.advance();
        self.items.push(ConversationItem::new(id, revision, state, content));
        id
    }

    fn update_tool(&mut self, index: usize, apply: impl FnOnce(&mut ToolCall)) {
        let item = &mut self.items[index];
        let rewrites = !item.is_open();
        let ConversationContent::Tool(tool_call) = &mut item.content else {
            return;
        };
        let previous = tool_call.clone();
        apply(tool_call);
        if *tool_call == previous {
            return;
        }
        item.state = if tool_call.rendering_final() { ItemState::Sealed } else { ItemState::Open };
        self.touch(index, rewrites);
    }

    fn touch(&mut self, index: usize, rewrites: bool) {
        let revision = self.advance();
        self.items[index].touch(revision, rewrites);
    }

    fn advance(&mut self) -> Revision {
        self.revision.advance()
    }
}
