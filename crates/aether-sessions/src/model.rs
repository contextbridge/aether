use aether_core::events::{AgentEvent, ContextEvent, LlmCallOutcome, MessageEvent, ModelEvent, ToolEvent, TurnEvent};
use llm::ContentBlock;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionMeta {
    pub session_id: String,
    pub cwd: PathBuf,
    pub model: String,
    #[serde(default)]
    pub selected_mode: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum UserEvent {
    Message { content: Vec<ContentBlock> },
    ClearContext,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum SessionControlEvent {
    AgentSwitched { from: Option<String>, to: Option<String> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "camelCase")]
#[allow(clippy::large_enum_variant)]
pub enum SessionEvent {
    User(UserEvent),
    Agent(AgentEvent),
    Control(SessionControlEvent),
}

impl SessionEvent {
    pub fn content(&self) -> Option<String> {
        self.user_content().or_else(|| match self {
            Self::Agent(event) => event.content(),
            Self::User(_) | Self::Control(_) => None,
        })
    }

    pub fn user_content(&self) -> Option<String> {
        match self {
            Self::User(UserEvent::Message { content }) => {
                let text = ContentBlock::join_text(content);
                (!text.is_empty()).then_some(text)
            }
            Self::User(UserEvent::ClearContext) | Self::Agent(_) | Self::Control(_) => None,
        }
    }

    pub fn is_persisted(&self) -> bool {
        match self {
            Self::User(_) | Self::Control(_) => true,
            Self::Agent(event) => match event {
                AgentEvent::Message(
                    MessageEvent::Text { is_complete, .. } | MessageEvent::Thought { is_complete, .. },
                ) => *is_complete,
                AgentEvent::Tool(
                    ToolEvent::Call { .. }
                    | ToolEvent::Result { .. }
                    | ToolEvent::Error { .. }
                    | ToolEvent::TaskCreated { .. }
                    | ToolEvent::TaskCompleted { .. }
                    | ToolEvent::TaskFailed { .. }
                    | ToolEvent::TaskCancelled { .. },
                )
                | AgentEvent::Turn(
                    TurnEvent::RetryScheduled { .. }
                    | TurnEvent::AutoContinue { .. }
                    | TurnEvent::Ended { .. }
                    | TurnEvent::LlmCallEnded { outcome: LlmCallOutcome::Failed { .. }, .. },
                )
                | AgentEvent::Context(
                    ContextEvent::CompactionStarted { .. }
                    | ContextEvent::CompactionEnded { .. }
                    | ContextEvent::CompactionResult { .. }
                    | ContextEvent::UsageUpdated { .. }
                    | ContextEvent::Cleared,
                )
                | AgentEvent::Model(ModelEvent::Switched { .. }) => true,
                AgentEvent::Tool(
                    ToolEvent::CallUpdate { .. }
                    | ToolEvent::ExecutionStarted { .. }
                    | ToolEvent::Progress { .. }
                    | ToolEvent::TaskStatus { .. }
                    | ToolEvent::DefinitionsUpdated { .. },
                )
                | AgentEvent::Turn(
                    TurnEvent::Started { .. }
                    | TurnEvent::LlmCallStarted { .. }
                    | TurnEvent::LlmCallEnded {
                        outcome: LlmCallOutcome::Completed { .. } | LlmCallOutcome::Cancelled,
                        ..
                    },
                ) => false,
            },
        }
    }
}

pub fn last_agent_from_events(initial: Option<String>, events: &[SessionEvent]) -> Option<String> {
    events
        .iter()
        .rev()
        .find_map(|event| match event {
            SessionEvent::Control(SessionControlEvent::AgentSwitched { to, .. }) => Some(to.clone()),
            _ => None,
        })
        .unwrap_or(initial)
}
