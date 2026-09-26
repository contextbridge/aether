use agent_client_protocol::schema::{MaybeUndefined, v2 as acp};
use schemars::JsonSchema;
use serde::Serialize;

/// What the agent is doing during the current turn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Activity {
    #[default]
    Idle,
    Thinking,
    Responding,
    RequiresAction,
    Working,
}

impl Activity {
    pub(super) fn after(update: &acp::SessionUpdate) -> Option<Self> {
        match update {
            acp::SessionUpdate::AgentMessageChunk(_)
            | acp::SessionUpdate::StateUpdate(acp::StateUpdate::Running(_)) => Some(Self::Responding),
            acp::SessionUpdate::StateUpdate(acp::StateUpdate::RequiresAction(_)) => Some(Self::RequiresAction),
            acp::SessionUpdate::ToolCallUpdate(_) => Some(Self::Working),
            acp::SessionUpdate::AgentThoughtChunk(chunk) => match &chunk.content {
                acp::ContentBlock::Text(text) if !text.text.is_empty() => Some(Self::Thinking),
                _ => None,
            },
            acp::SessionUpdate::AgentThought(message) => match &message.content {
                MaybeUndefined::Value(blocks) if blocks.iter().any(has_content) => Some(Self::Thinking),
                _ => None,
            },
            _ => None,
        }
    }
}

fn has_content(block: &acp::ContentBlock) -> bool {
    !matches!(block, acp::ContentBlock::Text(text) if text.text.is_empty())
}
