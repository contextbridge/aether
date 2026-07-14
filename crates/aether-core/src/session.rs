use crate::events::{AgentEvent, ContextEvent, LlmCallOutcome, MessageEvent, ModelEvent, ToolEvent, TurnEvent};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

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
    Message { content: Vec<llm::ContentBlock> },
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
                let text = llm::ContentBlock::join_text(content);
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
                AgentEvent::Tool(ToolEvent::Call { .. } | ToolEvent::Result { .. } | ToolEvent::Error { .. })
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

#[derive(Debug, Clone)]
pub struct SessionLine {
    pub line_number: usize,
    pub bytes_read: usize,
    pub raw: String,
}

#[derive(Debug)]
pub enum SessionLogEntry {
    Persisted { line: SessionLine, event: Box<SessionEvent> },
    Transient { line: SessionLine },
    Malformed { line: SessionLine, error: serde_json::Error },
}

impl SessionLogEntry {
    pub fn line(&self) -> &SessionLine {
        match self {
            Self::Persisted { line, .. } | Self::Transient { line } | Self::Malformed { line, .. } => line,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SessionLogError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("missing session metadata line")]
    MissingMetadata,
    #[error("invalid session metadata on line {line_number}: {source}")]
    InvalidMetadata { line_number: usize, source: serde_json::Error },
}

pub struct SessionLog<T: BufRead> {
    reader: T,
    pub meta: SessionMeta,
    line_number: usize,
}

impl SessionLog<BufReader<File>> {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, SessionLogError> {
        Self::from_reader(BufReader::new(File::open(path.as_ref())?))
    }
}

impl<T: BufRead> SessionLog<T> {
    pub fn from_reader(mut reader: T) -> Result<Self, SessionLogError> {
        let mut line = String::new();
        let mut line_number = 0;
        loop {
            line.clear();
            if reader.read_line(&mut line)? == 0 {
                return Err(SessionLogError::MissingMetadata);
            }
            line_number += 1;
            if !line.trim().is_empty() {
                break;
            }
        }
        let meta = serde_json::from_str(line.trim())
            .map_err(|source| SessionLogError::InvalidMetadata { line_number, source })?;
        Ok(Self { reader, meta, line_number })
    }

    pub fn next_entry(&mut self) -> std::io::Result<Option<SessionLogEntry>> {
        let Some(line) = self.next_line()? else {
            return Ok(None);
        };
        let entry = match serde_json::from_str::<SessionEvent>(&line.raw) {
            Ok(event) if event.is_persisted() => SessionLogEntry::Persisted { line, event: Box::new(event) },
            Ok(_) => SessionLogEntry::Transient { line },
            Err(error) => SessionLogEntry::Malformed { line, error },
        };
        Ok(Some(entry))
    }

    fn next_line(&mut self) -> std::io::Result<Option<SessionLine>> {
        let mut line = String::new();
        loop {
            line.clear();
            let bytes_read = self.reader.read_line(&mut line)?;
            if bytes_read == 0 {
                return Ok(None);
            }
            self.line_number += 1;
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                return Ok(Some(SessionLine { line_number: self.line_number, bytes_read, raw: trimmed.to_string() }));
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{LlmCallPurpose, TurnOutcome};

    fn agent(event: AgentEvent) -> SessionEvent {
        SessionEvent::Agent(event)
    }

    #[test]
    fn persistence_policy_covers_every_event_variant() {
        let retry = agent(AgentEvent::Turn(TurnEvent::RetryScheduled {
            purpose: LlmCallPurpose::Chat,
            attempt: 1,
            max_attempts: 3,
            delay_ms: 10,
        }));
        let cancelled = agent(AgentEvent::Turn(TurnEvent::Ended { outcome: TurnOutcome::Cancelled }));
        let partial = agent(AgentEvent::text("m", "partial", false));
        let compaction_ended = agent(AgentEvent::Context(ContextEvent::CompactionEnded {
            outcome: crate::events::CompactionOutcome::Completed,
        }));

        assert!(retry.is_persisted());
        assert!(cancelled.is_persisted());
        assert!(compaction_ended.is_persisted());
        assert!(!partial.is_persisted());
    }
}
