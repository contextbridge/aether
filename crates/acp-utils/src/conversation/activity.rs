use crate::content::map_content_blocks_to_text;
use agent_client_protocol::schema::{MaybeUndefined, v2 as acp};
use serde::Serialize;

/// What the agent is doing during the current turn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityPhase {
    #[default]
    Idle,
    Thinking,
    Responding,
    RequiresAction,
    Working,
}

/// The agent's activity within a turn, and the reasoning it is streaming while it thinks.
///
/// Activity is only tracked while a prompt is outstanding. Once a turn ends,
/// late activity (a stray thought, sub-agent progress, a compaction) is ignored
/// until the next turn starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Activity {
    phase: ActivityPhase,
    thought: String,
    #[serde(skip)]
    thought_message_id: Option<acp::MessageId>,
    #[serde(skip)]
    accepting: bool,
}

impl Default for Activity {
    fn default() -> Self {
        Self { phase: ActivityPhase::Idle, thought: String::new(), thought_message_id: None, accepting: true }
    }
}

impl Activity {
    pub fn phase(&self) -> ActivityPhase {
        self.phase
    }

    /// The current thought's full text, as streamed; empty unless the agent is thinking.
    pub fn thought(&self) -> &str {
        &self.thought
    }

    pub(super) fn accepting(&self) -> bool {
        self.accepting
    }

    pub(super) fn prompt_started(&mut self) {
        self.clear_thought();
        self.accepting = true;
        self.set_phase(ActivityPhase::Thinking);
    }

    pub(super) fn prompt_finished(&mut self) {
        self.clear_thought();
        self.set_phase(ActivityPhase::Idle);
        self.accepting = false;
    }

    pub(super) fn observe(&mut self, update: &acp::SessionUpdate) {
        match update {
            acp::SessionUpdate::AgentMessageChunk(_)
            | acp::SessionUpdate::StateUpdate(acp::StateUpdate::Running(_)) => {
                self.set_phase(ActivityPhase::Responding);
            }
            acp::SessionUpdate::StateUpdate(acp::StateUpdate::RequiresAction(_)) => {
                self.set_phase(ActivityPhase::RequiresAction);
            }
            acp::SessionUpdate::ToolCallUpdate(_) => self.set_phase(ActivityPhase::Working),
            acp::SessionUpdate::AgentThoughtChunk(chunk) => {
                if let acp::ContentBlock::Text(text) = &chunk.content
                    && !text.text.is_empty()
                {
                    self.record_thought(&chunk.message_id, &text.text);
                }
            }
            acp::SessionUpdate::AgentThought(message) => match &message.content {
                MaybeUndefined::Undefined => {}
                MaybeUndefined::Null => self.replace_thought(&message.message_id, ""),
                MaybeUndefined::Value(blocks) => {
                    self.replace_thought(&message.message_id, &map_content_blocks_to_text(blocks.clone()));
                }
            },
            _ => {}
        }
    }

    fn replace_thought(&mut self, message_id: &acp::MessageId, text: &str) {
        if text.is_empty() && self.thought_message_id.as_ref() != Some(message_id) {
            return;
        }
        self.clear_thought();
        if !text.is_empty() {
            self.record_thought(message_id, text);
        }
    }

    fn record_thought(&mut self, message_id: &acp::MessageId, chunk: &str) {
        if !self.accepting {
            return;
        }
        if self.thought_message_id.as_ref() != Some(message_id) {
            self.thought.clear();
            self.thought_message_id = Some(message_id.clone());
        }
        self.set_phase(ActivityPhase::Thinking);
        self.thought.push_str(chunk);
    }

    fn set_phase(&mut self, phase: ActivityPhase) {
        if phase != ActivityPhase::Idle && !self.accepting {
            return;
        }
        if self.phase == ActivityPhase::Thinking && phase != ActivityPhase::Thinking {
            self.clear_thought();
        }
        self.phase = phase;
    }

    fn clear_thought(&mut self) {
        self.thought.clear();
        self.thought_message_id = None;
    }
}
