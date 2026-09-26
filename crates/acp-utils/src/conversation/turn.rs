use agent_client_protocol::schema::v2 as acp;
use serde::Serialize;

/// Where the current prompt is in its lifecycle.
///
/// The agent answers `session/prompt` and reports `idle` independently, in
/// either order, so a turn can finish before its prompt is accepted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnPhase {
    /// No prompt is outstanding; a new one can be sent.
    #[default]
    Idle,
    /// A prompt was sent and the agent has not accepted it yet.
    Submitting,
    /// The agent accepted the prompt, or started a turn itself, and has not reached idle.
    Running,
    /// The agent reached idle before accepting the prompt, whose response is still owed.
    CompletedBeforeAcceptance,
}

/// A turn the conversation was waiting on reached idle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnFinished {
    pub stop_reason: Option<acp::StopReason>,
}

impl TurnPhase {
    pub fn is_idle(self) -> bool {
        self == Self::Idle
    }

    pub fn waiting_for_response(self) -> bool {
        matches!(self, Self::Submitting | Self::Running)
    }

    pub(super) fn accept(&mut self) {
        match self {
            Self::Submitting => *self = Self::Running,
            Self::CompletedBeforeAcceptance => *self = Self::Idle,
            Self::Idle | Self::Running => {}
        }
    }

    pub(super) fn finish(&mut self) {
        match self {
            Self::Submitting => *self = Self::CompletedBeforeAcceptance,
            Self::Running => *self = Self::Idle,
            Self::Idle | Self::CompletedBeforeAcceptance => {}
        }
    }
}
